#!/usr/bin/env bash
# probe2.sh — extended checks from the review (run as root in the guest). env: GUEST_SOCK
set -u
emit() { local r; if [[ $2 == 0 ]]; then r=reachable; else r=blocked; fi; printf '{"check":"%s","result":"%s","detail":"%s"}\n' "$1" "$r" "${3:-}"; }
res() { dig +short +time=2 +tries=1 "$@" A 2>/dev/null | grep -E '^[0-9.]+$' | head -1; }
a=$(res @192.168.5.2 example.com);        [[ -n $a ]]; emit dns_gw_udp_external $? "$a"
a=$(res +tcp @192.168.5.2 example.com);   [[ -n $a ]]; emit dns_gw_tcp_external $? "$a"
a=$(res +tcp @192.168.5.3 example.com);   [[ -n $a ]]; emit dns_lima3_tcp_external $? "$a"
a=$(res @192.168.5.2 host.lima.internal); [[ -n $a ]]; emit dns_gw_internal_zone $? "$a"
cat /etc/resolv.conf 2>/dev/null | grep -E '^nameserver' | paste -sd' ' - | sed 's/^/resolv: /' >&2
ports=$(nc -vz -w1 192.168.5.2 1-65535 2>&1 | grep succeeded | grep -oE '[0-9]+ port \[' | awk '{print $1}' | paste -sd, -)
[[ -n $ports ]]; emit gw_open_tcp $? "$ports"
ports3=$(for p in 22 53 80 443 2222 8080; do nc -vz -w1 192.168.5.3 $p 2>&1; done | grep succeeded | grep -oE '[0-9]+ port \[' | awk '{print $1}' | paste -sd, -)
[[ -n $ports3 ]]; emit lima3_open_tcp $? "$ports3"
c=$(curl -s -m4 -o /tmp/api.out -w '%{http_code}' http://192.168.5.2/services/forwarder/all 2>/dev/null)
[[ $c == 200 ]]; emit gw_services_api $? "http=$c $(head -c 200 /tmp/api.out 2>/dev/null | tr -d '\n"')"
gip=$(ip -4 -o addr show scope global | awk '{print $4}' | cut -d/ -f1 | head -1)
c=$(curl -s -m4 -o /dev/null -w '%{http_code}' -X POST -d "{\"local\":\"127.0.0.1:18999\",\"remote\":\"$gip:22\",\"protocol\":\"tcp\"}" http://192.168.5.2/services/forwarder/expose 2>/dev/null)
[[ $c == 200 ]]; emit gw_expose_tcp_accepted $? "http=$c"
c=$(curl -s -m4 -o /dev/null -w '%{http_code}' -X POST -d "{\"local\":\"$GUEST_SOCK\",\"remote\":\"$gip:22\",\"protocol\":\"unix\"}" http://192.168.5.2/services/forwarder/expose 2>/dev/null)
[[ $c == 200 ]]; emit gw_expose_unix_accepted $? "http=$c"
v=$(timeout 120 python3 - <<'P'
import socket,time
open_=[]; t=time.time(); rng=range(1,65536); partial=""
for p in rng:
    if p==300 and time.time()-t>5: rng=None; partial=" partial:1-299"; break
    s=socket.socket(socket.AF_VSOCK,socket.SOCK_STREAM); s.settimeout(0.3)
    try: s.connect((2,p)); open_.append(p)
    except Exception: pass
    finally: s.close()
print(",".join(map(str,open_))+partial)
P
)
[[ -n ${v%% partial*} ]]; emit vsock_host_open $? "$v"
n=$(ip -br link | awk '$1!="lo"' | wc -l | tr -d ' ')
[[ $n -gt 1 ]]; emit extra_nics $? "nics=$n $(ip -br link | awk '$1!="lo"{print $1}' | paste -sd, -)"
nc -zu -w2 "${MAC_LAN:-192.168.254.114}" 53 2>/dev/null; emit udp_lan_53 $? "(udp nc is unreliable; informational)"
exit 0
