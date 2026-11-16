set -euo pipefail
# bunker guest provisioning, run by `bunker up` as root through `limactl shell ... sudo bash -s`.
# Idempotent: every step checks first or rewrites a file only when its content changed. The DB
# password arrives on stdin just before this script (BUNKER_DB_PASSWORD) and is never echoed.
: "${BUNKER_DB_PASSWORD:?the DB password must come first on stdin}"
case $BUNKER_DB_PASSWORD in
*[!A-Za-z0-9]*) echo "bunker: the DB password must be alphanumeric" >&2; exit 1 ;;
esac

# The whole body is one function, parsed before anything runs: a cut-off stream runs nothing, and
# no command can read the rest of the script from stdin.
bunker_provision() {
PHP={{php}}
PROXY=http://127.0.0.1:3128
export DEBIAN_FRONTEND=noninteractive
export http_proxy=$PROXY https_proxy=$PROXY HTTP_PROXY=$PROXY HTTPS_PROXY=$PROXY
export no_proxy=localhost,127.0.0.1 NO_PROXY=localhost,127.0.0.1
GNUPGHOME=$(mktemp -d)
export GNUPGHOME
trap 'rm -rf "$GNUPGHOME"' EXIT
RESTART_NGINX=0
RELOAD_FPM=0
RELOAD_UNITS=0

# put <path> <mode> <owner:group>, content on stdin. Succeeds only when the file changed.
put() {
  local tmp
  tmp=$(mktemp)
  cat > "$tmp"
  if [ -f "$1" ] && cmp -s "$tmp" "$1"; then
    rm -f "$tmp"
    return 1
  fi
  install -D -m "$2" -o "${3%:*}" -g "${3#*:}" "$tmp" "$1"
  rm -f "$tmp"
}

# repo_key <name> <url> <fingerprint>: /etc/apt/keyrings/<name>.gpg, holding exactly the one key
# whose primary fingerprint is pinned. A mismatch aborts before anything is written.
repo_key() {
  local dst=/etc/apt/keyrings/$1.gpg tmp got
  if [ -f "$dst" ] && [ "$(key_fprs "$dst")" = "$3" ]; then
    return 0
  fi
  tmp=$(mktemp)
  curl -fsSL --retry 3 "$2" -o "$tmp"
  got=$(key_fprs "$tmp")
  if [ "$got" != "$3" ]; then
    echo "bunker: the $1 signing key from $2 has fingerprint '${got}', expected $3" >&2
    rm -f "$tmp"
    exit 1
  fi
  install -d -m 0755 /etc/apt/keyrings
  gpg --batch --yes --dearmor -o "$dst" "$tmp"
  chmod 0644 "$dst"
  rm -f "$tmp"
}

# The primary fingerprints in a key file, space-separated.
key_fprs() {
  gpg --batch --show-keys --with-colons "$1" 2>/dev/null |
    awk -F: '$1 == "pub" { want = 1; next } $1 == "fpr" && want { printf "%s%s", sep, $10; sep = " "; want = 0 }'
}

echo "== 1. hostname"
h=$(hostname)
if grep -q '^127\.0\.1\.1[[:space:]]' /etc/hosts; then
  sed -i "s/^127\.0\.1\.1[[:space:]].*/127.0.1.1 $h/" /etc/hosts
else
  echo "127.0.1.1 $h" >> /etc/hosts
fi

echo "== 2. proxy settings"
put /etc/environment 0644 root:root <<'BUNKER_EOF' || true
PATH="/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin:/usr/games:/usr/local/games:/snap/bin"
http_proxy=http://127.0.0.1:3128
https_proxy=http://127.0.0.1:3128
HTTP_PROXY=http://127.0.0.1:3128
HTTPS_PROXY=http://127.0.0.1:3128
no_proxy=localhost,127.0.0.1
NO_PROXY=localhost,127.0.0.1
BUNKER_EOF
put /etc/apt/apt.conf.d/95bunker-proxy 0644 root:root <<'BUNKER_EOF' || true
Acquire::http::Proxy "http://127.0.0.1:3128";
Acquire::https::Proxy "http://127.0.0.1:3128";
BUNKER_EOF

echo "== 3. automatic updates off (bunker's maintenance window runs them)"
systemctl disable --now apt-daily.timer apt-daily-upgrade.timer >/dev/null 2>&1 || true
# motd-news fetches motd.ubuntu.com, which the proxy blocks; off, it stops filling the net log.
systemctl disable --now motd-news.timer >/dev/null 2>&1 || true
put /etc/default/motd-news 0644 root:root <<'BUNKER_EOF' || true
ENABLED=0
BUNKER_EOF
put /etc/apt/apt.conf.d/20auto-upgrades 0644 root:root <<'BUNKER_EOF' || true
APT::Periodic::Update-Package-Lists "0";
APT::Periodic::Unattended-Upgrade "0";
BUNKER_EOF

echo "== 4. guest firewall"
if put /etc/nftables.conf 0755 root:root <<'BUNKER_EOF'; then
{{nftables}}
BUNKER_EOF
  if command -v nft >/dev/null; then nft -f /etc/nftables.conf; fi
fi

echo "== 5. users and layout"
id app >/dev/null 2>&1 || useradd --system --user-group --home-dir /nonexistent --no-create-home --shell /usr/sbin/nologin app
id deploy >/dev/null 2>&1 || useradd --user-group --create-home --shell /bin/bash deploy
install -d -m 0755 -o root -g root /srv/app /srv/app/shared
install -d -m 0755 -o deploy -g deploy /srv/app/releases
for d in storage storage/app storage/app/public storage/framework storage/framework/cache \
  storage/framework/sessions storage/framework/views storage/logs; do
  install -d -m 0755 -o app -g app "/srv/app/shared/$d"
done
put /home/deploy/.npmrc 0644 deploy:deploy <<'BUNKER_EOF' || true
proxy=http://127.0.0.1:3128
https-proxy=http://127.0.0.1:3128
BUNKER_EOF
# Composer has no proxy setting of its own; it reads HTTP(S)_PROXY from /etc/environment above.

echo "== 6. package repositories"
# Launchpad PPA for Ondrej Sury, fingerprint from https://launchpad.net/~ondrej/+archive/ubuntu/php
repo_key ondrej-php "https://keyserver.ubuntu.com/pks/lookup?op=get&options=mr&search=0x14AA40EC0831756756D7F66C4F4EA0AAE5267A6C" 14AA40EC0831756756D7F66C4F4EA0AAE5267A6C
put /etc/apt/sources.list.d/ondrej-php.list 0644 root:root <<'BUNKER_EOF' || true
deb [signed-by=/etc/apt/keyrings/ondrej-php.gpg] https://ppa.launchpadcontent.net/ondrej/php/ubuntu noble main
BUNKER_EOF
# Percona packaging key, fingerprint from https://docs.percona.com/percona-software-repositories/
# (it signs https://repo.percona.com/ps-84-lts/apt/dists/noble/Release). Oracle's repo.mysql.com
# has no arm64 packages for noble; Percona Server 8.4 LTS is the drop-in MySQL 8.4.
repo_key percona "https://keyserver.ubuntu.com/pks/lookup?op=get&options=mr&search=0x4D1BB29D63D98E422B2113B19334A25F8507EFA5" 4D1BB29D63D98E422B2113B19334A25F8507EFA5
put /etc/apt/sources.list.d/percona.list 0644 root:root <<'BUNKER_EOF' || true
deb [signed-by=/etc/apt/keyrings/percona.gpg] https://repo.percona.com/ps-84-lts/apt noble main
deb [signed-by=/etc/apt/keyrings/percona.gpg] https://repo.percona.com/telemetry/apt noble main
BUNKER_EOF
{{node_repo}}
apt-get update -q

echo "== 7. packages"
# Percona's telemetry agent is a hard dependency; masked before install, it never runs.
systemctl mask percona-telemetry-agent.service >/dev/null 2>&1 || true
install -d -m 0755 /etc/mysql/conf.d
put /etc/mysql/conf.d/bunker.cnf 0644 root:root <<'BUNKER_EOF' || true
[mysqld]
bind-address = 127.0.0.1
mysqlx = OFF
percona_telemetry_disable = ON
BUNKER_EOF
PERCONA_TELEMETRY_DISABLE=1 apt-get install -y -q --no-install-recommends {{packages}}
update-alternatives --set php "/usr/bin/php$PHP" >/dev/null
COMPOSER_VERSION={{composer_version}}
COMPOSER_SHA256={{composer_sha256}}
if ! echo "$COMPOSER_SHA256  /usr/local/bin/composer" | sha256sum -c --status 2>/dev/null; then
  tmp=$(mktemp)
  curl -fsSL --retry 3 "https://getcomposer.org/download/{{composer_version}}/composer.phar" -o "$tmp"
  if ! echo "$COMPOSER_SHA256  $tmp" | sha256sum -c --status; then
    echo "bunker: composer $COMPOSER_VERSION does not match its pinned sha256" >&2
    rm -f "$tmp"
    exit 1
  fi
  install -m 0755 "$tmp" /usr/local/bin/composer
  rm -f "$tmp"
fi

echo "== 8. php-fpm and nginx"
put "/etc/php/$PHP/fpm/pool.d/app.conf" 0644 root:root <<'BUNKER_EOF' && RELOAD_FPM=1
{{pool}}
BUNKER_EOF
if [ -e "/etc/php/$PHP/fpm/pool.d/www.conf" ]; then
  rm -f "/etc/php/$PHP/fpm/pool.d/www.conf"
  RELOAD_FPM=1
fi
put /etc/nginx/sites-available/app 0644 root:root <<'BUNKER_EOF' && RESTART_NGINX=1
{{nginx}}
BUNKER_EOF
if [ "$(readlink /etc/nginx/sites-enabled/app 2>/dev/null)" != /etc/nginx/sites-available/app ]; then
  ln -sfn /etc/nginx/sites-available/app /etc/nginx/sites-enabled/app
  RESTART_NGINX=1
fi
if [ -e /etc/nginx/sites-enabled/default ]; then
  rm -f /etc/nginx/sites-enabled/default
  RESTART_NGINX=1
fi
nginx -t -q

echo "== 9. database"
systemctl enable --now mysql >/dev/null
mysql --protocol=socket -uroot <<BUNKER_SQL
CREATE DATABASE IF NOT EXISTS app CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci;
CREATE USER IF NOT EXISTS 'app'@'localhost' IDENTIFIED BY '${BUNKER_DB_PASSWORD}';
CREATE USER IF NOT EXISTS 'app'@'127.0.0.1' IDENTIFIED BY '${BUNKER_DB_PASSWORD}';
ALTER USER 'app'@'localhost' IDENTIFIED BY '${BUNKER_DB_PASSWORD}';
ALTER USER 'app'@'127.0.0.1' IDENTIFIED BY '${BUNKER_DB_PASSWORD}';
GRANT ALL PRIVILEGES ON app.* TO 'app'@'localhost';
GRANT ALL PRIVILEGES ON app.* TO 'app'@'127.0.0.1';
BUNKER_SQL

echo "== 10. workers, scheduler, logrotate"
{{units}}
put /etc/logrotate.d/bunker-app 0644 root:root <<'BUNKER_EOF' || true
{{logrotate}}
BUNKER_EOF

echo "== 11. enable"
if [ "$RELOAD_UNITS" = 1 ]; then systemctl daemon-reload; fi
systemctl enable --now nftables nginx "php$PHP-fpm" mysql >/dev/null
if [ "$RELOAD_FPM" = 1 ]; then systemctl reload-or-restart "php$PHP-fpm"; fi
if [ "$RESTART_NGINX" = 1 ]; then systemctl restart nginx; fi
{{enable_units}}
echo "== provisioned"
}

bunker_provision </dev/null
