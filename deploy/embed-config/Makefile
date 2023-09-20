PUBLIC := andreiko/chargebot
PRIVATE := registry.fly.io/chargebot
VERSION ?= $(error VERSION is required)

.PHONY: all
all: build push

.PHONY: build
build:
	docker build -t $(PRIVATE):$(VERSION) --build-arg REPOSITORY=$(PUBLIC) --build-arg VERSION=$(VERSION) .

.PHONY: push
push:
	docker push $(PRIVATE):$(VERSION)
