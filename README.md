# clash-roam

Switch a [Clash](https://github.com/Dreamacro/clash) proxy selector when your IP address changes.

## Usage

```yaml
# config.yaml
external-controller: 127.0.0.1:9090
secret: 79d6d1bd-4bc2-4b5f-b12b-9471f29c140a
interface: wlan0
ip_min: 192.168.233.0
ip_max: 192.168.234.0
proxy_in: PROXY_1
proxy_out: PROXY_2
selector: PROXY
```

```console
$ clash-roam -c config.yaml
```

In absence of `-c` option, `clash-roam` tries to load `config.yaml` from the current directory.
