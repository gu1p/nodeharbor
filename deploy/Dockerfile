FROM gcr.io/distroless/cc-debian13:nonroot@sha256:c31ff9abcb1910f3ab25c7957bdaf0bfe12a01eb546e8df2282f1c8f682b606c
ARG TARGETARCH
ARG VERSION
ARG COMMIT
LABEL org.opencontainers.image.source="https://github.com/gu1p/nodeharbor" \
      org.opencontainers.image.version=$VERSION \
      org.opencontainers.image.revision=$COMMIT
COPY --chown=65532:65532 ${TARGETARCH}/nodeharbor-controller /nodeharbor-controller
COPY --chown=65532:65532 ${TARGETARCH}/nodeharbor-probe /nodeharbor-probe
COPY --chown=65532:65532 ${TARGETARCH}/ui /ui
USER 65532:65532
ENV NODEHARBOR_UI_DIR=/ui NODEHARBOR_LISTEN=0.0.0.0:8090 NODEHARBOR_DATABASE=sqlite:///data/nodeharbor.db
EXPOSE 8090 8091
ENTRYPOINT ["/nodeharbor-controller"]
