PUBLIC := andreiko/chargebot
PRIVATE := registry.fly.io/chargebot
VERSION ?= $(error VERSION is required)
PRIVATE_VERSION ?= $(VERSION)

.PHONY: all
all: build push

.PHONY: build
build:
	docker build -t $(PRIVATE):$(PRIVATE_VERSION) --build-arg REPOSITORY=$(PUBLIC) --build-arg VERSION=$(VERSION) .

.PHONY: push
push:
	docker push $(PRIVATE):$(PRIVATE_VERSION)
