# Deploying a nexus

How to run `shed-nexus` on a server of your own: as its own user, under
systemd with its sandboxing, behind nginx with a Let's Encrypt certificate, and
optionally with a ntfy server for notifications on the phone. [How Shed works](architecture.md#the-nexus)
describes what the nexus does.

Everything below uses placeholders: `nexus.example.com` for the nexus,
`ntfy.example.com` for ntfy, `admin@example.com` for the contact. Use your own.

**The trust model**: whoever controls the nexus can type into every linked
workspace's shells. Run it on a machine you trust, keep it updated, and keep its
data folder private.

## 1. Build and install

On the server (or build elsewhere for the same architecture):

```sh
cargo install --locked --git https://github.com/odolha/shed shed-nexus
sudo install -m 0755 ~/.cargo/bin/shed-nexus /usr/local/bin/
```

or from a checkout, `cargo build --release -p shed-nexus` and install
`target/release/shed-nexus`. It needs no C libraries at run time (SQLite and
TLS are built in).

The web app is built from the checkout's `web/` folder (Node 24.15 or newer;
see [web/README.md](../web/README.md)) into a folder the nexus serves:

```sh
cd web && npm ci && npm run build && cd ..
sudo install -d /usr/local/share/shed-nexus
sudo cp -r web/dist/shed-web /usr/local/share/shed-nexus/web
```

and `web_dir = "/usr/local/share/shed-nexus/web"` in the settings (below).
Without it, the nexus serves only the API.

## 2. A user of its own

The nexus never runs as root:

```sh
sudo useradd --system --home-dir /var/lib/shed-nexus --shell /usr/sbin/nologin shed-nexus
```

systemd makes `/var/lib/shed-nexus` (mode 0700, owned by `shed-nexus`) when the
service starts. The nexus keeps its database (`nexus.db`) and its push key
(`vapid.key`, 0600) there.

## 3. Settings

```sh
sudo install -d -m 0755 /etc/shed-nexus
sudo install -m 0644 packaging/nexus/nexus.toml /etc/shed-nexus/
sudoedit /etc/shed-nexus/nexus.toml
```

At least set `public_url = "https://nexus.example.com"`, and `web_dir` for
the web app. The sample
(`packaging/nexus/nexus.toml`) lists every setting with its default. With nginx
in front as below, keep `listen` on `127.0.0.1` and `trusted_proxy = true`: the
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
  ([7](#7-notifications-on-the-phone-with-ntfy-optional)).

Optional filters:

- `allowed_networks`: only these address ranges reach the nexus at all.
- `[geo]`: a country allow-list. Download a GeoIP CSV of ranges, such as db-ip's
  free "IP to Country Lite" (CSV, `start,end,country`), into the data folder,
  and set `database` and `countries`. Update the file monthly and restart. Without
  `countries`, the database still shows the country of each device and audit
  entry.

## 4. systemd

```sh
sudo install -m 0644 packaging/nexus/shed-nexus.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now shed-nexus
journalctl -u shed-nexus -f
```

The unit runs the nexus as `shed-nexus` with `NoNewPrivileges`, no
capabilities, `ProtectSystem=strict` (everything read-only but
`/var/lib/shed-nexus`), `ProtectHome`, private `/tmp` and devices, a system
call filter, and `MemoryMax`/`TasksMax` limits. If you move the data folder,
change `ReadWritePaths` and `StateDirectory` to match.

Until the account exists, the nexus writes a setup link to `setup-link.txt` in
its data folder (0600, readable by `shed-nexus` only) at every start, and logs
where it is, not the link itself:

```sh
sudo cat /var/lib/shed-nexus/setup-link.txt
# https://nexus.example.com/setup#code=…
```

The code is in the link's fragment (after `#`), which browsers never send, so
it does not end up in nginx's access log either. Open the link once nginx is
up (below): choose a user name and password, add the authenticator (scan the
QR code), confirm with a code from it, and keep the recovery codes somewhere
safe. The code is new at every start and works only until the account exists;
the file is removed then.

## 5. nginx and certbot

Install nginx and certbot (`sudo apt install nginx certbot python3-certbot-nginx`
on Debian and Ubuntu), point `nexus.example.com`'s DNS at the server, and add
`/etc/nginx/sites-available/shed-nexus`:

```nginx
server {
    listen 80;
    listen [::]:80;
    server_name nexus.example.com;

    location / {
        proxy_pass http://127.0.0.1:7870;
        proxy_http_version 1.1;
        proxy_set_header Host $host;
        # The client's address, replacing whatever the client sent; the
        # nexus believes it from loopback (trusted_proxy).
        proxy_set_header X-Forwarded-For $remote_addr;
        proxy_set_header X-Forwarded-Proto $scheme;
        client_max_body_size 1m;
    }

    # The WebSockets: workspaces' links and the apps.
    location ~ ^/(link|api)$ {
        proxy_pass http://127.0.0.1:7870;
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection "upgrade";
        proxy_set_header Host $host;
        proxy_set_header X-Forwarded-For $remote_addr;
        proxy_set_header X-Forwarded-Proto $scheme;
        # Sockets stay open; the nexus pings every 30 s.
        proxy_read_timeout 1h;
        proxy_send_timeout 1h;
    }
}
```

```sh
sudo ln -s /etc/nginx/sites-available/shed-nexus /etc/nginx/sites-enabled/
sudo nginx -t && sudo systemctl reload nginx
sudo certbot --nginx -d nexus.example.com --redirect -m admin@example.com --agree-tos
```

certbot adds the `listen 443 ssl` lines, the certificate, the redirect from
HTTP, and a timer that renews it. Open only ports 80 and 443 in the firewall;
port 7870 stays on loopback.

The nexus sets its own security headers (a strict Content-Security-Policy,
`X-Content-Type-Options`, `Referrer-Policy`, `X-Frame-Options`, and HSTS when
`public_url` is `https:`); nginx need not add them.

## 6. Linking workspaces

On each machine running Shed, `shed nexus join https://nexus.example.com`
prints a code such as `K7QF-2M9D`. In the web app (or the Android app), under
**Devices** → **Approve a Workspace…**, type that code (the list of waiting requests shows which machine asks and from where,
but not the code, so a request someone else made cannot be approved by
mistake), give the workspace a name and approve it with a code from your
authenticator. The workspace keeps its token in a 0600 file in Shed's data
folder (`nexus.json`, with the nexus's address: the link dials only that
nexus, and only over `https`, plain `http` being for a nexus on the same
machine). From then on the keeper runs the link (`shed link`) while Shed
runs; `shed nexus status` says how it is doing. Revoke the workspace under
Devices to cut it off at once; `shed nexus leave` unlinks it from its side. The
[user guide](user-guide.md#linking-a-workspace) has the rest (view-only
workspaces, private tabs).

## 7. Notifications on the phone with ntfy (optional)

While a Shed app is open and in view (the web app's page, or the Android app
in the foreground), notifications come to the apps directly and nothing is
pushed. When none is, browsers get Web Push through their vendor's push
service, with nothing to set up here, and phones get a message on a **ntfy
topic** you run, read by the ntfy app (the same way other projects' alerts
can come, one topic each: `alerts-shed`, `alerts-<project>`). Tapping one opens
its tab in Shed's app (`shed://w/…/b/…/t/…`).

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
sudo ntfy user add shed-bot                     # its password is not used: the nexus uses a token
sudo ntfy access shed-bot 'alerts-shed' write-only
sudo ntfy token add shed-bot                    # prints tk_…
sudo ntfy user add phone                        # (if not there yet) the phone signs in with it
sudo ntfy access phone 'alerts-*' read-only     # or 'alerts-shed'
sudo systemctl enable --now ntfy
```

The topic goes in `nexus.toml`:

```toml
[push]
ntfy_url = "https://ntfy.example.com/alerts-shed"
```

and the token in the service's env file, readable by root only (systemd reads
it for the nexus), never in `nexus.toml` or git. `SHED_NEXUS_NTFY_URL` there
wins over `ntfy_url` (`http://127.0.0.1:2586/alerts-shed` works too, with ntfy
on the same machine):

```sh
sudo install -m 600 /dev/null /etc/shed-nexus.env
echo 'SHED_NEXUS_NTFY_TOKEN=tk_…' | sudo tee -a /etc/shed-nexus.env >/dev/null
sudo systemctl restart shed-nexus
```

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
sudo ntfy access everyone 'up*' write-only
sudo ntfy access phone 'up*' read-only
```

A ntfy on a private network needs `allow_private_endpoints = true` in the
nexus's `[push]` (and `allow_http_endpoints = true` without TLS) for
UnifiedPush endpoints; the topic in `ntfy_url` may be anywhere.

## 8. A server workspace (optional)

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
and phones must subscribe to push again). To upgrade, install the new binary,
build the web app again and put it in place of the old folder (`sudo rm -r
/usr/local/share/shed-nexus/web` first, then the `cp` above), and `sudo
systemctl restart shed-nexus`; the database is migrated when it opens.
Workspaces reconnect on their own.
