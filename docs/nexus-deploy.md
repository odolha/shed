# Deploying a nexus

How to run `shed-nexus` on a server of your own, on Ubuntu or Debian: one
binary (the web app is built into it) and one script, which sets it up as its
own user under systemd with its sandboxing, behind nginx with a Let's Encrypt
certificate. Optionally a ntfy server brings notifications to the phone.
[How Shed works](architecture.md#the-nexus) describes what the nexus does.

Everything below uses placeholders: `nexus.example.com` for the nexus,
`ntfy.example.com` for ntfy, `admin@example.com` for the contact. Use your own.

**The trust model**: whoever controls the nexus can type into every linked
workspace's shells. Run it on a machine you trust, keep it updated, and keep its
data folder private.

In short, as root on the server:

```sh
curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal && . ~/.cargo/env
curl -fsSL https://raw.githubusercontent.com/nvm-sh/nvm/v0.40.3/install.sh | bash && . ~/.nvm/nvm.sh
nvm install 24
apt install -y build-essential git
cargo install --locked --git https://github.com/odolha/shed shed-nexus
git clone https://github.com/odolha/shed /root/shed
/root/shed/packaging/nexus/install.sh DOMAIN=nexus.example.com EMAIL=admin@example.com
```

and open the setup link it prints. The steps, one by one:

## 0. DNS

Point `nexus.example.com` at the server: an `A` record with its IPv4 address
(and `AAAA` for IPv6). certbot needs it to resolve before step 3. Open ports
80 and 443 in the provider's firewall; the nexus itself stays on loopback.

## 1. Rust and Node

Building needs Rust (1.95 or newer) and, for the web app built into the
binary, Node 24.15 or newer (or 22.22.3+) with npm. With
[rustup](https://rustup.rs) and [nvm](https://github.com/nvm-sh/nvm):

```sh
curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal && . ~/.cargo/env
curl -fsSL https://raw.githubusercontent.com/nvm-sh/nvm/v0.40.3/install.sh | bash && . ~/.nvm/nvm.sh
nvm install 24
apt install -y build-essential git      # a C compiler (SQLite is built in) and git
```

Node is needed only to build; the server never runs it. Without Node, build
with `SHED_NEXUS_NO_WEB=1`: the nexus then serves only the API (the Android
app and workspaces' links work; the browser app does not, unless `web_dir`
points at a build of `web/`, see [Settings](#settings)).

## 2. Build shed-nexus

```sh
cargo install --locked --git https://github.com/odolha/shed shed-nexus
```

This builds the web app (`npm ci` and `npm run build`, in cargo's build folder)
and the nexus, which carries it, into `~/.cargo/bin/shed-nexus`. It needs no
C libraries at run time (SQLite and TLS are built in), so it can also be built
on another machine of the same architecture and copied over. From a checkout,
`cargo build --release -p shed-nexus` gives `target/release/shed-nexus`.

## 3. Run the script

```sh
git clone https://github.com/odolha/shed /root/shed
/root/shed/packaging/nexus/install.sh DOMAIN=nexus.example.com EMAIL=admin@example.com
```

With a ntfy topic for phones ([5](#5-notifications-on-the-phone-with-ntfy-optional)),
add `NTFY_URL=https://ntfy.example.com/alerts-shed`. `BIN=/path/to/shed-nexus`
installs another binary than the one `cargo install` made; `DRY_RUN=1` only
prints what it would do. The script installs nginx and certbot if missing,
installs the binary to `/usr/local/bin`, makes the `shed-nexus` user, the
settings, the secrets file, the systemd service and the nginx site, gets the
certificate, starts the nexus and prints the setup link. It is safe to run
again: it never overwrites the settings, the secrets or the nginx site once
they exist ([What the script did](#what-the-script-did) has the details).

## 4. The account

Open the setup link the script printed
(`https://nexus.example.com/setup#code=…`): choose a user name and password,
add the authenticator (scan the QR code, or type in the key; Authy's 7-digit
codes are taken too), confirm with a code from it, and keep
the recovery codes somewhere safe (each signs you in once if the authenticator
is lost). The link is new at every start of the nexus and works only until the
account exists. Later, to see it again:

```sh
cat /var/lib/shed-nexus/setup-link.txt
```

The nexus keeps it in that file (0600, its own) rather than in its log, and
the code is in the link's fragment (after `#`), which browsers never send, so
it stays out of nginx's access log too.

## 5. Notifications on the phone with ntfy (optional)

While a Shed app is open and in view (the web app's page, or the Android app
in the foreground), notifications come to the apps directly and nothing is
pushed. When none is, browsers get Web Push through their vendor's push
service, with nothing to set up here, and phones get a message on a **ntfy
topic** you run, read by the ntfy app (the same way other projects' alerts can
come, one topic each: `alerts-shed`, `alerts-<project>`). Tapping one opens its
tab in Shed's app (`shed://w/…/b/…/t/…`).

Install the ntfy server (see ntfy's documentation for its package repository),
then in `/etc/ntfy/server.yml`:

```yaml
base-url: "https://ntfy.example.com"
listen-http: "127.0.0.1:2586"
behind-proxy: true
cache-file: "/var/cache/ntfy/cache.db"
cache-duration: "24h"
auth-file: "/var/lib/ntfy/user.db"
auth-default-access: "deny-all"
```

Put it behind nginx like the nexus (its own `server_name ntfy.example.com`,
`proxy_pass http://127.0.0.1:2586`, the WebSocket headers on `location /`,
`proxy_buffering off`, `proxy_read_timeout 3m`) and run certbot for it.

Accounts: one publisher per project, write-only on its topic, with an access
token; the phone reads (all `alerts-*` topics, or just `alerts-shed`); nobody
else can do either (`deny-all`):

```sh
ntfy user add shed-bot                     # its password is not used: the nexus uses a token
ntfy access shed-bot 'alerts-shed' write-only
ntfy token add shed-bot                    # prints tk_…
ntfy user add phone                        # (if not there yet) the phone signs in with it
ntfy access phone 'alerts-*' read-only     # or 'alerts-shed'
systemctl enable --now ntfy
```

The topic is `ntfy_url` under `[push]` in `/etc/shed-nexus/nexus.toml` (the
script writes it given `NTFY_URL`; otherwise add it by hand). The token goes in
the service's env file, readable by root only (systemd reads it for the
nexus), never in `nexus.toml` or git; the script made the file:

```sh
echo 'SHED_NEXUS_NTFY_TOKEN=tk_…' >> /etc/shed-nexus.env
systemctl restart shed-nexus
```

`SHED_NEXUS_NTFY_URL` in that file wins over `ntfy_url`
(`http://127.0.0.1:2586/alerts-shed` works too, with ntfy on the same machine).
The nexus POSTs each notification to the topic with `Authorization: Bearer`
and the token: the text in the body (workspace, bench and tab), `Title`,
`Priority` (4 when Claude waits for an answer or a workspace went offline, 3
otherwise), `Tags`, and `Click` with the tab's link. Failures are logged
(`ntfy: http 403`). The topic is shared by every phone, so it carries the kinds
any signed-in phone has on in Shed's settings (all, before a phone chooses).
The message text is not encrypted, which is why the topic belongs on a ntfy
you run, closed to others. Try it:

```sh
curl -H "Authorization: Bearer tk_…" -H "Title: test" -H "Priority: 4" -d hello https://ntfy.example.com/alerts-shed
```

On the phone, install the ntfy app, set its default server to
`https://ntfy.example.com`, sign in as `phone` (Settings → Users) and
subscribe to `alerts-shed`. Shed's app says the same in its Settings →
Notifications, with the topic to copy; with a topic it does not register for
UnifiedPush.

**Without a topic: UnifiedPush.** Leave `ntfy_url` out (with a ntfy you do not
run, say) and Shed's app registers through the ntfy app as its UnifiedPush
distributor instead, handing its endpoint to the nexus, which sends it the same
encrypted Web Push requests as browsers get (ntfy sees only ciphertext). The
nexus publishes to those `up…` topics without an account, so that ntfy must let
anyone publish to them, UnifiedPush's usual set-up (safe enough: the topics'
names are long and random and only the phone can decrypt what is sent):

```sh
ntfy access everyone 'up*' write-only
ntfy access phone 'up*' read-only
```

A ntfy on a private network needs `allow_private_endpoints = true` in the
nexus's `[push]` (and `allow_http_endpoints = true` without TLS) for
UnifiedPush endpoints; the topic in `ntfy_url` may be anywhere.

## 6. Linking workspaces

On each machine running Shed, `shed nexus join https://nexus.example.com`
prints a code such as `K7QF-2M9D`. In the web app (or the Android app), under
**Devices** → **Approve a Workspace…**, type that code (the list of waiting
requests shows which machine asks and from where, but not the code, so a
request someone else made cannot be approved by mistake), give the workspace a
name and approve it with a code from your authenticator. The workspace keeps
its token in a 0600 file in Shed's data folder (`nexus.json`, with the nexus's
address: the link dials only that nexus, and only over `https`, plain `http`
being for a nexus on the same machine). From then on the keeper runs the link
(`shed link`) while Shed runs; `shed nexus status` says how it is doing.
Revoke the workspace under Devices to cut it off at once; `shed nexus leave`
unlinks it from its side. The [user guide](user-guide.md#linking-a-workspace)
has the rest (view-only workspaces, private tabs).

Shed's desktop app can also show other linked workspaces in its own windows
(`shed nexus signin` on that desktop, and `[nexus] desktop_clients = true` in
the shown workspace's `shed.toml`; see the [user
guide](user-guide.md#other-workspaces-in-this-window)). Nothing is needed on the
nexus for it beyond running a version that has it: the desktop shows in
Devices as a **Shed desktop**, and each time it opens a workspace the audit log
says so. A reverse proxy in front must pass WebSocket binary messages through
(the usual WebSocket proxying does).

## 7. The phone

In a browser, `https://nexus.example.com` is the web app; it can be added to
the home screen. The Android app ([android/README.md](../android/README.md)
has how to build and install it) asks for the nexus's address, the user name,
the password and a code; then, for notifications while it is closed, the ntfy
app as in [5](#5-notifications-on-the-phone-with-ntfy-optional).

## What the script did

`packaging/nexus/install.sh`, step by step, with what to do by hand on a
system it does not cover:

- **Packages**: `apt install nginx certbot python3-certbot-nginx curl`, if
  missing.
- **The binary**: `install -m 0755 ~/.cargo/bin/shed-nexus /usr/local/bin/`
  (each run, if it changed).
- **The update command**: `shed-nexus-update` in `/usr/local/sbin`, a link to
  `packaging/nexus/update.sh` in the checkout.
- **The user**: `useradd --system --home-dir /var/lib/shed-nexus --shell
  /usr/sbin/nologin shed-nexus`. The nexus never runs as root. systemd makes
  `/var/lib/shed-nexus` (0700, owned by `shed-nexus`) when the service starts;
  the nexus keeps its database (`nexus.db`) and its push key (`vapid.key`,
  0600) there.
- **Settings**: `/etc/shed-nexus/nexus.toml` from the sample
  (`packaging/nexus/nexus.toml`), with `public_url =
  "https://nexus.example.com"`, `trusted_proxy = true`, `[push] subject =
  "mailto:admin@example.com"` and `ntfy_url` if given. Only when there is none:
  after that it is yours to edit.
- **Secrets**: `/etc/shed-nexus.env`, 0600 root, empty but for a comment, if
  missing.
- **systemd**: `packaging/nexus/shed-nexus.service` to
  `/etc/systemd/system/` (each run), `systemctl daemon-reload`, `enable`,
  `restart`. The unit runs the nexus as `shed-nexus` with `NoNewPrivileges`, no
  capabilities, `ProtectSystem=strict` (everything read-only but
  `/var/lib/shed-nexus`), `ProtectHome`, private `/tmp` and devices, a system
  call filter, and `MemoryMax`/`TasksMax` limits. If you move the data folder,
  change `ReadWritePaths` and `StateDirectory` to match. Its log:
  `journalctl -u shed-nexus -f`.
- **nginx**: `packaging/nexus/nginx-site.conf` with your domain as
  `/etc/nginx/sites-available/shed-nexus`, linked into `sites-enabled`, then
  `nginx -t` and a reload. Only when there is none (certbot edits it). It
  passes the client's address in `X-Forwarded-For`, which the nexus believes
  from loopback only, upgrades `/link` and `/api` to WebSockets with long
  timeouts, and compresses the web app's files. With ufw on, `ufw allow 'Nginx
  Full'`.
- **Certificate**: `certbot --nginx -d nexus.example.com --redirect -m
  admin@example.com --agree-tos --non-interactive`, unless the site has one.
  certbot adds the `listen 443 ssl` lines, the certificate, the redirect from
  HTTP, and a timer that renews it.
- **Start**: waits for the nexus to answer and prints the setup link from
  `/var/lib/shed-nexus/setup-link.txt` (or says the account exists).

The nexus sets its own security headers (a strict Content-Security-Policy,
`X-Content-Type-Options`, `Referrer-Policy`, `X-Frame-Options`, and HSTS when
`public_url` is `https:`); nginx need not add them.

## Settings

The sample (`packaging/nexus/nexus.toml`) lists every setting with its default;
edit `/etc/shed-nexus/nexus.toml` and `systemctl restart shed-nexus`. With
nginx in front, keep `listen` on `127.0.0.1` and `trusted_proxy = true`: the
nexus then believes `X-Forwarded-For` from loopback only (set
`trusted_proxies` to the proxy's addresses if it runs elsewhere) and takes the
client to be the right-most address in it that is not a trusted proxy. Without
it, every client would look like the proxy, and one address's failed sign-ins
would lock out everyone.

Limits worth knowing (all in the sample):

- `[login]`: failed sign-ins lock out their address (an IPv4 address or an
  IPv6 /64) for `lockout_minutes`. With `global_max_failures` failures from
  everywhere, the nexus slows down rather than locking everyone out: addresses
  that never signed in share `slow_per_minute` attempts, while those your
  devices signed in or were seen from are not held back. Wrong one-time codes
  from a signed-in device (revoking, pairing, the password) are counted per
  device, apart from sign-ins.
- `max_connections` and `header_timeout_secs`: connections at once, and how
  long a client may take to send a request's headers.
- Push endpoints must be on public addresses (`[push] allow_private_endpoints`
  for a ntfy on your network); `[push] ntfy_url` is a ntfy topic for phones
  ([5](#5-notifications-on-the-phone-with-ntfy-optional)).

Optional:

- `web_dir`: serve the web app from a folder (a build of `web/`, `npm run
  build` → `web/dist/shed-web`) instead of the one built in, for a nexus built
  with `SHED_NEXUS_NO_WEB=1` or a web app of another version.
- `allowed_networks`: only these address ranges reach the nexus at all.
- `[geo]`: a country allow-list. Download a GeoIP CSV of ranges, such as db-ip's
  free "IP to Country Lite" (CSV, `start,end,country`), into the data folder,
  and set `database` and `countries`. Update the file monthly and restart. Without
  `countries`, the database still shows the country of each device and audit
  entry.

## A server workspace (optional)

A server can run Shed itself, so benches and Claude Code keep working while
your PCs are off: a keeper for a user of its own, reached with `shed tui` over
ssh, and linked to the nexus. It needs no display: build
Shed without windows. The placeholders here are `server.example.com` and the
user `shed`.

```sh
# A user of its own, with a shell (you will ssh in as it), lingering so its
# systemd services run without a login and survive logouts.
sudo useradd --create-home --shell /bin/bash shed
sudo loginctl enable-linger shed

# Shed without windows: the CLI, shed tui, and the keeper.
cargo build --release -p shed-app --no-default-features --features tui
cargo build --release -p shed-keeper
sudo install -m 0755 target/release/shed target/release/shedd /usr/local/bin/
```

As `shed` (`sudo -iu shed`, or `ssh shed@server.example.com`), run the keeper
as a user service that stays up with nothing open, and sign Claude Code in
once (its usage limits are the account's, shown on every workspace using it):

```sh
mkdir -p ~/.config/systemd/user
# packaging/linux/shedd.service from the checkout, pointed at where shedd is:
sed 's|/usr/bin/shedd|/usr/local/bin/shedd|' shedd.service > ~/.config/systemd/user/shedd.service
systemctl --user enable --now shedd.service
printf '[keeper]\nstay = true\n' >> ~/.config/shed/shed.toml   # also when started by hand
claude login
```

Then, from anywhere:

```sh
ssh -t shed@server.example.com shed       # a build without windows runs shed tui
```

`Ctrl+B d`, or a dropped connection, leaves the interface; everything keeps
running for the next `ssh`. [The user guide](user-guide.md#shed-in-a-terminal-shed-tui)
has its keys. Remote tabs from your PCs to this server (`shed remote new
shed@server.example.com`) use the same keeper, so they show up there as
benches of their own. To reach it from the web and Android apps too, `shed nexus join
https://nexus.example.com` links the workspace as in [6](#6-linking-workspaces).

## Backups and upgrades

Back up `/var/lib/shed-nexus` (`nexus.db` with `sqlite3 nexus.db ".backup
/path/to/nexus.db"` while it runs, and `vapid.key`; without the key, browsers
and phones must subscribe to push again) and `/etc/shed-nexus*`.

To upgrade, as root on the server:

```sh
shed-nexus-update
```

It pulls the checkout (`/root/shed`), builds `shed-nexus` from it, and runs
`install.sh` with it. (`install.sh` makes that command, a link in
`/usr/local/sbin` to `packaging/nexus/update.sh`; on a nexus set up before it
existed, run `git -C /root/shed pull && /root/shed/packaging/nexus/update.sh`
once.) Or from your computer, which checks your commits are pushed and runs it
over ssh:

```sh
tools/deploy-nexus.sh root@nexus.example.com    # or SHED_NEXUS_SSH=root@… in your shell's profile
```
 If the
server also has a server workspace (`/usr/local/bin/shedd` and a user `shed`,
or `WORKSPACE_USER`), it builds and installs its `shed` and `shedd` too, and
asks before restarting that keeper (`-y` restarts without asking,
`--no-restart` never, `--no-workspace` leaves it alone): a restart saves its
benches but ends what runs in them, Claude Code sessions included.

Step by step instead: build the new version and run the script again (it
installs the binary, refreshes the unit and restarts; the domain comes from
the settings):

```sh
cargo install --locked --git https://github.com/odolha/shed shed-nexus
git -C /root/shed pull
/root/shed/packaging/nexus/install.sh
```

or by hand, `install -m 0755 ~/.cargo/bin/shed-nexus /usr/local/bin/` and
`systemctl restart shed-nexus`. The web app comes with the binary; the
database is migrated when it opens. Workspaces reconnect on their own.
