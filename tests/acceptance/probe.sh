#!/usr/bin/env bash
# probe.sh [--root-tamper] <proxy-url>   (runs inside the guest; env: LOOP_PORT ALL_PORT PROXY_PORT MAC_LAN ROUTER TS_PEER TS_SELF)
set -u
if [[ ${1:-} == --root-tamper ]]; then
  shift
  [[ $(id -u) == 0 ]] || { echo "probe: --root-tamper needs root" >&2; exit 2; }
  nft flush ruleset 2>/dev/null
  ip route replace default via 192.168.5.2 2>/dev/null
  rm -f /etc/resolv.conf; printf 'nameserver 192.168.5.3\n' > /etc/resolv.conf
  echo "probe: tampered (nft flushed, default via 192.168.5.2, resolv.conf -> 192.168.5.3)" >&2
fi
PROXY=${1:?proxy url}
emit() { if [[ $2 == 0 ]]; then r=reachable; else r=blocked; fi; printf '{"check":"%s","result":"%s"}\n' "$1" "$r"; }
code() { [[ $(curl -sS -m4 -x "$PROXY" -o /dev/null -w '%{http_code}' "$1" 2>/dev/null) == 200 ]]; }
addr() { [[ -n $(dig +short +time=2 +tries=1 "@$1" example.com A 2>/dev/null | grep -E '^[0-9.]+$') ]]; }
code https://example.com;                                   emit proxy_allowed $?
code https://github.com;                                    emit proxy_denied $?
nc -z -w4 1.1.1.1 443 2>/dev/null;                          emit direct_tcp_internet $?
curl -sS -m4 -k -o /dev/null https://1.1.1.1 2>/dev/null;   emit direct_https_internet $?
addr 1.1.1.1;                                               emit direct_udp_dns $?
addr 192.168.5.3;                                           emit lima_dns $?
timeout 5 getent hosts example.com >/dev/null 2>&1;         emit system_resolver $?
curl -sS -m4 -o /dev/null "http://192.168.5.2:$LOOP_PORT" 2>/dev/null; emit host_loopback_other_port $?
nc -z -w4 192.168.5.2 "$PROXY_PORT" 2>/dev/null;            emit proxy_via_lima_ip $?
curl -sS -m4 -o /dev/null "http://$MAC_LAN:$ALL_PORT" 2>/dev/null;     emit host_lan_ip $?
nc -z -w4 "$MAC_LAN" 22 2>/dev/null;                        emit host_lan_ssh $?
nc -z -w4 "$ROUTER" 80 2>/dev/null;                         emit router $?
nc -z -w4 "$TS_PEER" 22 2>/dev/null;                        emit tailnet_peer $?
nc -z -w4 "$TS_SELF" 22 2>/dev/null;                        emit tailnet_self $?
curl -6 -sS -m4 -k -o /dev/null 'https://[2606:4700:4700::1111]' 2>/dev/null; emit ipv6_internet $?
ping -c1 -W2 1.1.1.1 >/dev/null 2>&1;                       emit icmp_internet $?
exit 0
