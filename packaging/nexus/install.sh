#!/usr/bin/env bash
# Sets up a Shed nexus on an Ubuntu or Debian server, or updates one: the
# shed-nexus binary, its user, settings and systemd service, nginx in front
# with a Let's Encrypt certificate (certbot). See docs/nexus-deploy.md.
#
# As root, from a checkout of the repository, after
# `cargo install --locked --git https://github.com/odolha/shed shed-nexus`:
#
#   packaging/nexus/install.sh DOMAIN=nexus.example.com EMAIL=admin@example.com
#   packaging/nexus/install.sh DOMAIN=… EMAIL=… NTFY_URL=https://ntfy.example.com/alerts-shed
#   packaging/nexus/install.sh                 # again later: installs a newer binary, restarts
#
#   DOMAIN    the nexus's name (its DNS A record points at this server); on a
#             second run, taken from nexus.toml
#   EMAIL     for Let's Encrypt and as the push contact; needed until there is
#             a certificate
#   NTFY_URL  optional: a ntfy topic for phones (written into a new nexus.toml)
#   BIN       the shed-nexus to install; default: the one cargo install made
#             (~/.cargo/bin, /root/.cargo/bin, or the sudo user's)
#   DRY_RUN=1 only print what it would do (needs no root)
#
# Arguments may also be given as environment variables. Safe to run again: it
# never overwrites the settings (/etc/shed-nexus/nexus.toml), the secrets
# (/etc/shed-nexus.env) or the nginx site once they exist; it refreshes the
# binary and the systemd unit, and restarts the service.
set -euo pipefail

usage() { sed -n '2,/^set -euo/p' "$0" | sed '$d; s/^# \{0,1\}//'; }

for arg in "$@"; do
  case "$arg" in
    DOMAIN=* | EMAIL=* | NTFY_URL=* | BIN=* | DRY_RUN=*) printf -v "${arg%%=*}" '%s' "${arg#*=}" ;;
    -h | --help) usage; exit 0 ;;
    *) echo "unknown argument: $arg (see --help)" >&2; exit 2 ;;
  esac
done
DOMAIN=${DOMAIN:-}
EMAIL=${EMAIL:-}
NTFY_URL=${NTFY_URL:-}
BIN=${BIN:-}
DRY_RUN=${DRY_RUN:-}

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
repo=$(cd "$here/../.." && pwd)

CONF_DIR=/etc/shed-nexus
CONF=$CONF_DIR/nexus.toml
ENV_FILE=/etc/shed-nexus.env
UNIT=/etc/systemd/system/shed-nexus.service
SITE=/etc/nginx/sites-available/shed-nexus
DATA=/var/lib/shed-nexus
TARGET_BIN=/usr/local/bin/shed-nexus

say() { printf '\n\033[1;36m== %s\033[0m\n' "$*"; }
note() { printf '   %s\n' "$*"; }
die() { printf '\n\033[1;31mshed-nexus install: %s\033[0m\n' "$*" >&2; exit 1; }
dry() { [[ -n "$DRY_RUN" && "$DRY_RUN" != 0 ]]; }

# Runs a command, or says it would.
run() {
  if dry; then
    printf '   would run:'; printf ' %q' "$@"; printf '\n'
  else
    "$@"
  fi
}

# Writes stdin to a file with a mode (atomically), or says it would.
put() {
  local path=$1 mode=$2 tmp
  if dry; then
    printf '   would write %s (%s):\n' "$path" "$mode"
    sed 's/^/     | /'
    return
  fi
  tmp=$(mktemp "$path.XXXXXX")
  cat > "$tmp"
  chmod "$mode" "$tmp"
  mv -f "$tmp" "$path"
}

# ------------------------------------------------------------------ checks

for file in nexus.toml shed-nexus.service nginx-site.conf; do
  [[ -f "$here/$file" ]] || die "$here/$file is missing: run this script from a checkout of the repository"
done
if ! dry; then
  [[ $EUID -eq 0 ]] || die "run it as root (sudo $0 …)"
  command -v apt-get >/dev/null || die "this script is for Debian and Ubuntu (apt); docs/nexus-deploy.md has the steps by hand"
fi

# A second run takes the domain from the settings.
if [[ -z "$DOMAIN" && -f "$CONF" ]]; then
  DOMAIN=$(sed -n 's|^public_url *= *"https\{0,1\}://\([^/":]*\).*|\1|p' "$CONF" | head -n 1)
fi
[[ -n "$DOMAIN" ]] || die "DOMAIN=nexus.example.com is needed (see --help)"
domain_re='^[A-Za-z0-9]([A-Za-z0-9.-]*[A-Za-z0-9])?$'
email_re='^[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+$'
ntfy_re='^https?://[A-Za-z0-9.:-]+(/[A-Za-z0-9._~/-]*)?$'
[[ "$DOMAIN" =~ $domain_re && "$DOMAIN" == *.* ]] || die "DOMAIN=$DOMAIN is not a domain name"
[[ -z "$EMAIL" || "$EMAIL" =~ $email_re ]] || die "EMAIL=$EMAIL is not an email address"
[[ -z "$NTFY_URL" || "$NTFY_URL" =~ $ntfy_re ]] \
  || die "NTFY_URL=$NTFY_URL is not a ntfy topic URL (https://ntfy.example.com/alerts-shed)"

if [[ -z "$EMAIL" && ! -f "$CONF" ]]; then
  die "EMAIL=admin@example.com is needed for new settings (the push contact) and the certificate"
fi

# The binary: BIN, else what cargo install made.
if [[ -z "$BIN" ]]; then
  candidates=("${CARGO_HOME:-$HOME/.cargo}/bin/shed-nexus" /root/.cargo/bin/shed-nexus)
  if [[ -n "${SUDO_USER:-}" ]]; then
    sudo_home=$(getent passwd "$SUDO_USER" | cut -d: -f6)
    [[ -n "$sudo_home" ]] && candidates+=("$sudo_home/.cargo/bin/shed-nexus")
  fi
  for candidate in "${candidates[@]}"; do
    if [[ -x "$candidate" ]]; then BIN=$candidate; break; fi
  done
fi
if [[ -n "$BIN" ]]; then
  [[ -x "$BIN" ]] || die "BIN=$BIN is not an executable"
elif [[ ! -x "$TARGET_BIN" ]]; then
  die "no shed-nexus found: build it first, with Node 24.15+ on PATH (for the web app):
   cargo install --locked --git https://github.com/odolha/shed shed-nexus
or give its path: BIN=/path/to/shed-nexus"
fi

dry && say "DRY_RUN: nothing is changed"

# ------------------------------------------------------------------ steps

say "packages: nginx, certbot"
missing=()
for pkg in nginx certbot python3-certbot-nginx curl; do
  dpkg -s "$pkg" >/dev/null 2>&1 || missing+=("$pkg")
done
if ((${#missing[@]})); then
  run apt-get update -q
  run env DEBIAN_FRONTEND=noninteractive apt-get install -y -q "${missing[@]}"
else
  note "all there"
fi

say "the binary: $TARGET_BIN"
if [[ -z "$BIN" ]]; then
  note "kept the one there (no newer one found)"
elif [[ -f "$TARGET_BIN" ]] && cmp -s "$BIN" "$TARGET_BIN"; then
  note "already the one in $BIN"
else
  run install -m 0755 "$BIN" "$TARGET_BIN"
  note "from $BIN"
fi

say "a user of its own: shed-nexus"
if id -u shed-nexus >/dev/null 2>&1; then
  note "there already"
else
  run useradd --system --home-dir "$DATA" --shell /usr/sbin/nologin shed-nexus
fi

say "settings: $CONF"
run install -d -m 0755 "$CONF_DIR"
if [[ -f "$CONF" ]]; then
  note "kept as it is (edit it by hand; this script never overwrites it)"
  if [[ -n "$NTFY_URL" ]] && ! grep -q '^ *ntfy_url *=' "$CONF"; then
    note "NTFY_URL is not in it: add  ntfy_url = \"$NTFY_URL\"  under [push], then: systemctl restart shed-nexus"
  fi
else
  {
    sed -e "s|^public_url = .*|public_url = \"https://$DOMAIN\"|" \
      -e "s|^# subject = \"mailto:.*|subject = \"mailto:$EMAIL\"|" \
      "$here/nexus.toml" |
      if [[ -n "$NTFY_URL" ]]; then
        sed -e "s|^# ntfy_url = .*|ntfy_url = \"$NTFY_URL\"|"
      else
        cat
      fi
  } | put "$CONF" 0644
  note "public_url = https://$DOMAIN, trusted_proxy = true (nginx in front), push subject mailto:$EMAIL${NTFY_URL:+, ntfy_url = $NTFY_URL}"
fi

say "secrets: $ENV_FILE"
if [[ -f "$ENV_FILE" ]]; then
  note "kept as it is"
else
  put "$ENV_FILE" 0600 <<'EOF'
# Secrets for the Shed nexus, read by systemd for shed-nexus.service; readable
# by root only. The ntfy publishing token, if nexus.toml has an ntfy_url:
# SHED_NEXUS_NTFY_TOKEN=tk_...
# then: systemctl restart shed-nexus
EOF
fi

say "systemd: shed-nexus.service"
run install -m 0644 "$here/shed-nexus.service" "$UNIT"
if [[ -f "$repo/docs/nexus-deploy.md" ]]; then
  run install -D -m 0644 "$repo/docs/nexus-deploy.md" /usr/local/share/doc/shed-nexus/nexus-deploy.md
fi
run systemctl daemon-reload
run systemctl enable --quiet shed-nexus
run systemctl restart shed-nexus

say "nginx: $SITE"
listen=127.0.0.1:7870
if [[ -f "$CONF" ]]; then
  configured=$(sed -n 's|^listen *= *"\([^"]*\)".*|\1|p' "$CONF" | head -n 1)
  [[ -n "$configured" ]] && listen=$configured
fi
if [[ -f "$SITE" ]]; then
  note "kept as it is (certbot may have added HTTPS to it)"
  grep -qF "$DOMAIN" "$SITE" || note "warning: it does not name $DOMAIN"
else
  sed -e "s|nexus\.example\.com|$DOMAIN|g" -e "s|127\.0\.0\.1:7870|$listen|g" "$here/nginx-site.conf" | put "$SITE" 0644
fi
if [[ ! -e /etc/nginx/sites-enabled/shed-nexus ]]; then
  run ln -s "$SITE" /etc/nginx/sites-enabled/shed-nexus
fi
run nginx -t -q
if systemctl is-active --quiet nginx; then
  run systemctl reload nginx
else
  run systemctl enable --now nginx
fi
if command -v ufw >/dev/null && ufw status 2>/dev/null | grep -q '^Status: active'; then
  note "ufw is on: letting HTTP and HTTPS in"
  run ufw allow 'Nginx Full'
fi

say "certificate: Let's Encrypt for $DOMAIN"
if [[ -f "$SITE" ]] && grep -q ssl_certificate "$SITE"; then
  note "already set up (certbot renews it by itself)"
else
  if [[ -f "/etc/letsencrypt/live/$DOMAIN/fullchain.pem" ]]; then
    account=(--keep-until-expiring)
  elif [[ -n "$EMAIL" ]]; then
    account=(-m "$EMAIL")
  else
    die "EMAIL=admin@example.com is needed for the certificate"
  fi
  if ! run certbot --nginx -d "$DOMAIN" --redirect --agree-tos --non-interactive "${account[@]}"; then
    die "certbot failed. Is $DOMAIN's DNS A record pointing at this server, and are ports 80 and 443 open?
Then run this script again."
  fi
fi

say "starting"
if dry; then
  note "would wait for the nexus on $listen, then print the setup link from $DATA/setup-link.txt"
  exit 0
fi
for _ in $(seq 60); do
  curl -sf "http://$listen/api/v1/status" >/dev/null && break
  sleep 0.5
done
if ! curl -sf "http://$listen/api/v1/status" >/dev/null; then
  journalctl -u shed-nexus -n 30 --no-pager >&2 || true
  die "the nexus does not answer on $listen (journalctl -u shed-nexus)"
fi
note "$("$TARGET_BIN" --version) is up"
code=$(curl -s -o /dev/null -w '%{http_code}' "http://$listen/")
if [[ "$code" == 404 ]] && ! grep -q '^ *web_dir *=' "$CONF"; then
  note "warning: this shed-nexus has no web app built in (built with SHED_NEXUS_NO_WEB, or without Node?);"
  note "the API works, the browser app does not. docs/nexus-deploy.md, step 1."
fi

if grep -qs '^ *ntfy_url *=' "$CONF" && ! grep -qs '^SHED_NEXUS_NTFY_TOKEN=' "$ENV_FILE"; then
  say "ntfy"
  note "nexus.toml has a ntfy topic but $ENV_FILE has no token yet: add"
  note "  SHED_NEXUS_NTFY_TOKEN=tk_…   (from: ntfy token add shed-bot)"
  note "to it, then: systemctl restart shed-nexus"
fi

say "done"
if [[ -f "$DATA/setup-link.txt" ]]; then
  note "Open this link to make the account (password, authenticator, recovery codes):"
  note ""
  note "  $(cat "$DATA/setup-link.txt")"
  note ""
  note "It works until the account exists, and changes at every restart."
else
  note "The account exists: sign in at https://$DOMAIN"
fi
note "Link each PC with: shed nexus join https://$DOMAIN"
