VERSION ?= $(error VERSION is required)
PRIVATE_VERSION ?= $(VERSION)

.PHONY: all
all:

.PHONY: deploy
deploy: embed-config deploy

.PHONY: embed-config
embed-config:
	$(MAKE) -C embed-config all

.PHONY: deploy
deploy:
	fly deploy -i registry.fly.io/chargebot:$(PRIVATE_VERSION)
