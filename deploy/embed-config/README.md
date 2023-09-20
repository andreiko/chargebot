# embed-config

This makefile pulls the public image, adds a config file to it and pushes the result to a private repository
for deployment.

Although not ideal, it's a decent way to provide configuration when deploying to simpler environments that don't support
configuration file mounting.

## Example

```bash
$ cp ../../example-config.yaml config.yaml
$ vim config.yaml
$ make VERSION=abcdeabc
```
