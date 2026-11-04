// Served with its datastar import rewritten to the fingerprinted URL, so the
// page and this module share one Datastar instance.
import { action, mergePatch } from "./datastar.js";

// The countdown and the rollover banner both read `$_now`. Datastar has no
// interval attribute in this build, so the clock ticks from here.
mergePatch({ _now: Date.now() });
setInterval(() => mergePatch({ _now: Date.now() }), 1000);

// `data-init="@modal(el)"` and `data-on:click="@modal('#stats')"`. The dialog
// body takes focus rather than the close button, so opening on page load does
// not paint a keyboard focus ring.
action({
  name: "modal",
  apply(_ctx, target) {
    const dialog = typeof target === "string" ? document.querySelector(target) : target;
    if (!(dialog instanceof HTMLDialogElement) || dialog.open) return;
    dialog.showModal();
    dialog.querySelector("[data-modal-body]")?.focus();
  },
});

// ─── Suggestions placement ───
//
// The list opens below the field when there is room and above it otherwise,
// measured against the visual viewport: on iOS window.innerHeight does not
// shrink when the keyboard opens, so it would put the list under the keyboard.

const GAP = 6;
const MIN_LIST_HEIGHT = 132;
const MAX_LIST_HEIGHT = 320;

function place() {
  const input = document.getElementById("guess");
  const list = document.getElementById("suggest-wrap");
  if (!input || !list) return;
  const viewport = window.visualViewport;
  const top = viewport?.offsetTop ?? 0;
  const height = viewport?.height ?? window.innerHeight;
  const rect = input.getBoundingClientRect();
  const below = top + height - rect.bottom - GAP;
  const above = rect.top - top - GAP;
  const side = below >= MIN_LIST_HEIGHT || below >= above ? "below" : "above";
  list.dataset.side = side;
  list.style.maxHeight = `${Math.max(MIN_LIST_HEIGHT, Math.min(MAX_LIST_HEIGHT, side === "below" ? below : above))}px`;
}

for (const type of ["focusin", "input"]) document.addEventListener(type, place);
window.addEventListener("resize", place);
window.visualViewport?.addEventListener("resize", place);
window.visualViewport?.addEventListener("scroll", place);

// ─── Sharing ───

function note(el, message, bad = false) {
  const target = el.closest("[data-share-root]")?.querySelector("[data-share-note]");
  if (!target) return;
  target.textContent = message;
  target.classList.toggle("text-warn", bad);
}

async function copy(text) {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    return false;
  }
}

// The OS share sheet where there is one, the clipboard where there is not.
action({
  name: "shareText",
  async apply({ el }, text) {
    if ("share" in navigator) {
      try {
        await navigator.share({ text });
        note(el, "Shared");
        return;
      } catch (error) {
        // Closing the sheet is a choice, not a failure.
        if (error instanceof Error && error.name === "AbortError") return;
      }
    }
    if (await copy(text)) note(el, "Copied — paste it anywhere");
    else note(el, "Your browser blocked that. Try Copy instead.", true);
  },
});

action({
  name: "copyText",
  async apply({ el }, text) {
    if (await copy(text)) note(el, "Copied — paste it anywhere");
    else note(el, "Your browser blocked the clipboard.", true);
  },
});

// noopener, so the opened window cannot navigate this one.
action({
  name: "openShare",
  apply({ el }, href) {
    window.open(href, "_blank", "noopener,noreferrer");
    note(el, " ");
  },
});

action({
  name: "shareImage",
  async apply({ el }, json) {
    const card = JSON.parse(json);
    // Before any await: Safari only allows a clipboard write inside the gesture.
    // Stories cannot take a link with the image, so it goes on the clipboard
    // for a Link sticker.
    const linkCopied = await copy(card.link);
    const label = el.querySelector("span");
    if (label) label.textContent = "Making…";
    const blob = await renderCard(card);
    if (label) label.textContent = "Image";
    if (!blob) return note(el, "The image could not be made.", true);

    const file = new File([blob], card.filename, { type: "image/png" });
    if (typeof navigator.canShare === "function" && navigator.canShare({ files: [file] })) {
      const payloads = [
        { files: [file], text: card.caption, url: card.link },
        { files: [file], text: card.link },
        { files: [file] },
      ];
      const payload = payloads.find((p) => navigator.canShare(p)) ?? { files: [file] };
      try {
        await navigator.share(payload);
      } catch (error) {
        if (!(error instanceof Error && error.name === "AbortError")) {
          return note(el, "The image could not be shared.", true);
        }
      }
      return note(
        el,
        linkCopied
          ? "The link is on your clipboard — add a Link sticker in Stories and paste it."
          : "The address is printed on the image.",
      );
    }

    const url = URL.createObjectURL(blob);
    const link = document.createElement("a");
    link.href = url;
    link.download = card.filename;
    link.click();
    setTimeout(() => URL.revokeObjectURL(url), 10_000);
    note(el, "Image saved to your downloads.");
  },
});

// ─── The Stories card ───
//
// 1080×1920, drawn as tiles rather than emoji, which render differently on
// every platform. Only the band of each guess reaches the browser, never the
// station, so the card cannot carry the answer.

const W = 1080;
const H = 1920;
const DISCS = ["#E32017", "#0098D4", "#00782A", "#B36305", "#6950A1", "#EF3340"];
const PAPER = "#fdfcfa";
const INK = "#171512";
const INK_SOFT = "#6f6a60";
const GO = "#00682a";
const TILE_EMPTY = "#e4dfd6";

function rounded(ctx, x, y, w, h, r) {
  ctx.beginPath();
  ctx.moveTo(x + r, y);
  ctx.arcTo(x + w, y, x + w, y + h, r);
  ctx.arcTo(x + w, y + h, x, y + h, r);
  ctx.arcTo(x, y + h, x, y, r);
  ctx.arcTo(x, y, x + w, y, r);
  ctx.closePath();
  ctx.fill();
}

function draw(ctx, card) {
  ctx.fillStyle = PAPER;
  ctx.fillRect(0, 0, W, H);

  const letters = [...card.title.toUpperCase()];
  const radius = 46;
  const gap = 20;
  const total = letters.length * radius * 2 + (letters.length - 1) * gap;
  const startX = (W - total) / 2;
  ctx.fillStyle = INK;
  rounded(ctx, startX - 34, 295, total + 68, 10, 5);
  letters.forEach((letter, i) => {
    const cx = startX + radius + i * (radius * 2 + gap);
    ctx.beginPath();
    ctx.arc(cx, 300, radius, 0, Math.PI * 2);
    ctx.fillStyle = PAPER;
    ctx.fill();
    ctx.lineWidth = 10;
    ctx.strokeStyle = DISCS[i % DISCS.length];
    ctx.stroke();
    ctx.fillStyle = INK;
    ctx.font = "900 44px Archivo, sans-serif";
    ctx.textAlign = "center";
    ctx.textBaseline = "middle";
    ctx.fillText(letter, cx, 302);
  });

  ctx.textAlign = "center";
  ctx.textBaseline = "middle";
  ctx.fillStyle = INK_SOFT;
  ctx.font = "700 34px Archivo, sans-serif";
  ctx.fillText(card.puzzle === null ? "PRACTICE ROUND" : `NO. ${card.puzzle}`, W / 2, 412);

  // Score and grid centred together, so a three-row win has no void above it.
  const scoreHeight = 150;
  const scoreToGrid = 90;
  const gridHeight = card.rows.length * 120 - 16;
  const blockTop = 500 + (1040 - (scoreHeight + scoreToGrid + gridHeight)) / 2;

  ctx.fillStyle = card.won ? GO : INK;
  ctx.font = "900 190px Archivo, sans-serif";
  const score = card.won ? `${card.rows.length}/${card.maxGuesses}` : `X/${card.maxGuesses}`;
  ctx.fillText(score, W / 2, blockTop + scoreHeight / 2);

  const tile = 104;
  const tileGap = 16;
  const gridX = (W - (5 * tile + 4 * tileGap)) / 2;
  const gridY = blockTop + scoreHeight + scoreToGrid;
  card.rows.forEach((filled, row) => {
    for (let col = 0; col < 5; col++) {
      ctx.fillStyle = col < filled ? GO : TILE_EMPTY;
      rounded(ctx, gridX + col * (tile + tileGap), gridY + row * (tile + tileGap), tile, tile, 18);
    }
  });

  if (card.streak >= 2 && card.puzzle !== null) {
    ctx.fillStyle = INK_SOFT;
    ctx.font = "700 40px Archivo, sans-serif";
    ctx.fillText(`${card.streak} day streak`, W / 2, 1580);
  }

  ctx.fillStyle = INK;
  ctx.font = "800 46px Archivo, sans-serif";
  ctx.fillText(card.tagline, W / 2, 1690);

  ctx.fillStyle = INK_SOFT;
  ctx.font = '600 40px "Spline Sans Mono", monospace';
  ctx.fillText(card.url.replace(/^https?:\/\//, ""), W / 2, 1770);
}

async function renderCard(card) {
  const canvas = document.createElement("canvas");
  canvas.width = W;
  canvas.height = H;
  const ctx = canvas.getContext("2d");
  if (!ctx) return null;
  // Canvas does not wait for webfonts, so the first card would otherwise draw
  // in a fallback face.
  try {
    await Promise.all([
      document.fonts.load("900 190px Archivo"),
      document.fonts.load("800 46px Archivo"),
      document.fonts.load("700 34px Archivo"),
      document.fonts.load('600 40px "Spline Sans Mono"'),
    ]);
  } catch {
    // Draw in the fallback face.
  }
  draw(ctx, card);
  return new Promise((resolve) => canvas.toBlob(resolve, "image/png"));
}
