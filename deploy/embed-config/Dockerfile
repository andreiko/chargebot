ARG REPOSITORY
ARG VERSION
FROM ${REPOSITORY}:${VERSION}
ENTRYPOINT [ "/chargebot", "-c", "config.yaml" ]
COPY config.yaml /
