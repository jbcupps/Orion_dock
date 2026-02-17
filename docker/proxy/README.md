# Orion Dual Proxy Configuration

This directory contains the dual-proxy egress boundary used by the `full` profile in `docker/docker-compose.yml`.

## Layout

- `internal/squid.conf` - Internal forward proxy (`proxy_internal:3128`)
- `external/squid.conf.template` - External policy/egress proxy template (`proxy_external:3129`)
- `external/entrypoint.sh` - Renders final external config from env toggles
- `external/allowlist_domains.txt` - Domain allowlist source file for `PROXY_MODE=allowlist`

## Traffic Flow

`app services -> proxy_internal -> proxy_external -> host/internet`

## Security Defaults

- No TLS interception (CONNECT passthrough only)
- Allowed destination ports: `80`, `443`
- CONNECT allowed only to `443`
- Denied destination ranges:
  - `10.0.0.0/8`
  - `172.16.0.0/12`
  - `192.168.0.0/16`
  - `127.0.0.0/8`
  - `169.254.0.0/16`

## Runtime Toggles (external proxy)

- `PROXY_MODE=allow_all|allowlist`
  - `allow_all` (default): permit destinations after SSRF and port rules
  - `allowlist`: only permit domains in `allowlist_domains.txt`
- `PROXY_ALLOW_HOST_DOCKER_INTERNAL=true|false`
  - `true` (default): allow `host.docker.internal` explicitly before private-range denies
- `PROXY_EXTRA_SAFE_PORTS`
  - Optional comma-separated safe ports, e.g. `8443,8080`
- `PROXY_ALLOWLIST_FILE`
  - Path to allowlist file (default `/etc/squid/allowlist_domains.txt`)

## Logs

With the full profile running, logs are mounted to:

- `./.orion/proxy/internal/access.log`
- `./.orion/proxy/external/access.log`

The configured format is:

`timestamp client_ip method destination status bytes`
