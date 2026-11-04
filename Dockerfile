# tubele as a container. Packages a binary built outside it: CI builds the
# static musl binary per architecture (the `linux-build` job) and builds this
# with dist/ as the context, holding <arch>/tubele. By hand:
# `docker build -f Dockerfile dist`.
#
# No CA bundle: the service makes no outbound requests.

FROM scratch

ARG TARGETARCH

COPY ${TARGETARCH}/tubele /tubele

EXPOSE 8080
ENV BIND_ADDR=0.0.0.0:8080
ENV DATABASE_PATH=/data/tubele.db

USER 65532:65532

LABEL org.opencontainers.image.title="tubele"
LABEL org.opencontainers.image.source="https://github.com/radiosilence/tubele-rs"

ENTRYPOINT ["/tubele"]
