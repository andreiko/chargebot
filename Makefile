TAG := $(shell git rev-parse --short HEAD)
RUST_IMAGE := rust:1.72.0-bookworm

IMAGE := chargebot
REPOSITORY := 023582495064.dkr.ecr.us-west-2.amazonaws.com

.PHONY: all
all:

.PHONY: image
image:
	docker run -it --rm -w /crate/ -v $(shell pwd):/crate/ $(RUST_IMAGE) cargo build --release
	docker build -t $(REPOSITORY)/$(IMAGE):$(TAG) .

.PHONY: push
push:
	docker push $(REPOSITORY)/$(IMAGE):$(TAG)
