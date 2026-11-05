#!/usr/bin/env sh
# Prints this machine's LAN addresses as host:port pairs for EUNOMIA_LAN_ADDRS (peer vaults,
# docs/peer-vaults.md): the address the default route leaves from first, then any other private
# IPv4 address (RFC 1918, link-local, Tailscale's 100.64.0.0/10). Docker bridges are left out.
#
#   scripts/lan-addrs.sh            -> 192.168.1.20:3000,100.101.1.2:3000
#   scripts/lan-addrs.sh --env .env -> adds EUNOMIA_LAN_ADDRS=... to .env unless it is already set
#
# The port is FRONTEND_PORT (default 3000): other Eunomias reach this one through its frontend.
set -eu

port="${FRONTEND_PORT:-3000}"
envfile=""
if [ "${1:-}" = "--env" ]; then envfile="${2:-.env}"; fi

lan() {
  case "$1" in
    10.*|192.168.*|169.254.*) return 0 ;;
    172.1[6-9].*|172.2[0-9].*|172.3[0-1].*) return 0 ;;
    100.6[4-9].*|100.[7-9][0-9].*|100.1[01][0-9].*|100.12[0-7].*) return 0 ;;
  esac
  return 1
}

candidates() {
  if command -v ip >/dev/null 2>&1; then
    # the default route's source address first
    ip -4 route get 1.1.1.1 2>/dev/null | sed -n 's/.* src \([0-9.]*\).*/\1/p'
    # then every global address, skipping docker and bridge interfaces
    ip -4 -o addr show scope global 2>/dev/null | awk '$2 !~ /^(docker|br-|veth|virbr)/ {split($4, a, "/"); print a[1]}'
  elif command -v ipconfig >/dev/null 2>&1; then
    # macOS
    for i in en0 en1 en2 utun0 utun1 utun2 utun3; do ipconfig getifaddr "$i" 2>/dev/null || true; done
  elif command -v hostname >/dev/null 2>&1; then
    hostname -I 2>/dev/null | tr ' ' '\n'
  fi
}

addrs=""
for ip in $(candidates); do
  lan "$ip" || continue
  case ",$addrs," in *",$ip:$port,"*) continue ;; esac
  addrs="${addrs:+$addrs,}$ip:$port"
done

if [ -n "$envfile" ]; then
  if [ -f "$envfile" ] && grep -q '^EUNOMIA_LAN_ADDRS=.' "$envfile"; then
    exit 0
  fi
  [ -n "$addrs" ] || exit 0
  touch "$envfile"
  sed -i.bak '/^EUNOMIA_LAN_ADDRS=/d' "$envfile" && rm -f "$envfile.bak"
  echo "EUNOMIA_LAN_ADDRS=$addrs" >> "$envfile"
  echo "==> other Eunomias on this network can reach this one at: $addrs"
else
  echo "$addrs"
fi
