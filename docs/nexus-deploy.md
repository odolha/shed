# Deploying a nexus

How to run `shed-nexus` on a server of your own: as its own user, under
systemd with its sandboxing, behind nginx with a Let's Encrypt certificate, and
optionally with a ntfy server for push to Android. [How Shed works](architecture.md#the-nexus)
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

The web app, once built, goes in a folder the nexus serves, such as
`/usr/local/share/shed-nexus/web`.

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

At least set `public_url = "https://nexus.example.com"`. The sample
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
  for a ntfy on your network).

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
prints a code such as `K7QF-2M9D`. In the web app, under devices, type that
code (the list of waiting requests shows which machine asks and from where,
but not the code, so a request someone else made cannot be approved by
mistake), give the workspace a name and approve it with a code from your
authenticator. The workspace keeps its token in a 0600 file in Shed's data
folder (`nexus.json`, with the nexus's address: the link dials only that
nexus, and only over `https`, plain `http` being for a nexus on the same
machine). Revoke it there to cut it off at once.

## 7. Push to Android with ntfy (optional)

Browsers get Web Push through their vendor's push service, with nothing to
set up here. The Android app uses UnifiedPush, through a distributor app on the
phone; ntfy is one, and you can host its server too. The nexus sends the same
encrypted Web Push requests either way, so ntfy only ever sees ciphertext.

Install the ntfy server (see ntfy's documentation for its package repository),
then in `/etc/ntfy/server.yml`:

```yaml
base-url: "https://ntfy.example.com"
listen-http: "127.0.0.1:2586"
behind-proxy: true
auth-file: "/var/lib/ntfy/user.db"
auth-default-access: "deny-all"
```

Let anyone publish to UnifiedPush topics (their names are long and random; the
nexus publishes without an account), and give the phone an account to read
them:

```sh
sudo ntfy access everyone 'up*' write-only
sudo ntfy user add phone
sudo ntfy access phone 'up*' read-only
sudo systemctl enable --now ntfy
```

Put it behind nginx like the nexus (its own `server_name ntfy.example.com`,
`proxy_pass http://127.0.0.1:2586`, the WebSocket headers on `location /`,
`proxy_read_timeout 1h`) and run certbot for it. On the phone, install the ntfy
app, set its default server to `https://ntfy.example.com` and sign in as
`phone`; Shed's app then registers through it and hands its endpoint to the
nexus. A ntfy on a private network needs `allow_private_endpoints = true` in
the nexus's `[push]` (and `allow_http_endpoints = true` without TLS).

## Backups and upgrades

Back up `/var/lib/shed-nexus` (`nexus.db` with `sqlite3 nexus.db ".backup
/path/to/nexus.db"` while it runs, and `vapid.key`; without the key, browsers
and phones must subscribe to push again). To upgrade, install the new binary
and `sudo systemctl restart shed-nexus`; the database is migrated when it
opens. Workspaces reconnect on their own.
