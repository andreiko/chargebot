FROM debian:bookworm
EXPOSE 8000
ENTRYPOINT [ "/chargebot", "-c", "config.yaml" ]
RUN apt-get update -y && apt-get install -y ca-certificates
COPY target/release/chargebot /
