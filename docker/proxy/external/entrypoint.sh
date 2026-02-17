#!/bin/sh
set -eu

TEMPLATE_PATH="${SQUID_TEMPLATE_PATH:-/etc/squid/squid.conf.template}"
OUTPUT_PATH="${SQUID_OUTPUT_PATH:-/etc/squid/squid.conf}"
PROXY_MODE="${PROXY_MODE:-allow_all}"
PROXY_ALLOW_HOST_DOCKER_INTERNAL="${PROXY_ALLOW_HOST_DOCKER_INTERNAL:-true}"
PROXY_ALLOWLIST_FILE="${PROXY_ALLOWLIST_FILE:-/etc/squid/allowlist_domains.txt}"
PROXY_EXTRA_SAFE_PORTS="${PROXY_EXTRA_SAFE_PORTS:-}"

case "$PROXY_MODE" in
  allow_all|allowlist) ;;
  *)
    echo "Invalid PROXY_MODE: $PROXY_MODE (expected allow_all or allowlist)" >&2
    exit 1
    ;;
esac

tmp_dir="$(mktemp -d)"
host_rules_file="$tmp_dir/host_rules.conf"
mode_rules_file="$tmp_dir/mode_rules.conf"
extra_ports_file="$tmp_dir/extra_ports.conf"

cleanup() {
  rm -rf "$tmp_dir"
}
trap cleanup EXIT

allow_host=false
case "$PROXY_ALLOW_HOST_DOCKER_INTERNAL" in
  true|TRUE|1|yes|YES|on|ON) allow_host=true ;;
esac

if [ "$allow_host" = "true" ]; then
  cat > "$host_rules_file" <<'EOF'
acl host_gateway dstdomain host.docker.internal
http_access allow host_gateway
EOF
else
  : > "$host_rules_file"
fi

if [ -n "$PROXY_EXTRA_SAFE_PORTS" ]; then
  old_ifs="$IFS"
  IFS=', '
  set -- $PROXY_EXTRA_SAFE_PORTS
  IFS="$old_ifs"
  : > "$extra_ports_file"
  for port in "$@"; do
    case "$port" in
      ''|*[!0-9]*) continue ;;
      *) printf 'acl Safe_ports port %s\n' "$port" >> "$extra_ports_file" ;;
    esac
  done
else
  : > "$extra_ports_file"
fi

if [ "$PROXY_MODE" = "allowlist" ]; then
  cat > "$mode_rules_file" <<EOF
acl allowed_domains dstdomain "$PROXY_ALLOWLIST_FILE"
http_access allow allowed_domains
http_access deny all
EOF
else
  cat > "$mode_rules_file" <<'EOF'
http_access allow all
http_access deny all
EOF
fi

awk \
  -v host_file="$host_rules_file" \
  -v mode_file="$mode_rules_file" \
  -v extra_file="$extra_ports_file" \
  '
  function emit(path, line) {
    while ((getline line < path) > 0) print line
    close(path)
  }
  /__HOST_ACCESS_RULES__/ { emit(host_file); next }
  /__MODE_RULES__/ { emit(mode_file); next }
  /__EXTRA_SAFE_PORTS__/ { emit(extra_file); next }
  { print }
  ' \
  "$TEMPLATE_PATH" > "$OUTPUT_PATH"

mkdir -p /var/log/squid
touch /var/log/squid/access.log
# Bind mounts can default to restrictive ownership/modes on some hosts.
# Keep this world-writable so Squid's daemon logger can append reliably.
chmod 0777 /var/log/squid || true
chmod 0666 /var/log/squid/access.log || true

exec squid -N -f "$OUTPUT_PATH"
