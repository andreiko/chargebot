FROM ubuntu:jammy
EXPOSE 8000
RUN apt-get update -y && apt-get install -y ca-certificates
COPY target/release/chargebot /
