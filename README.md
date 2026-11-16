# TohdohBoard Backend (Server)

A local REST API backend for the TohdohBoard ecosystem, built with Rust and Axum.

## Overview
This server allows the `tohdohboard-web` and `tohdohboard-cli` clients to sync state using a single source of truth. It reads and writes the shared `schema.json` compliant data to the local disk.

## Running Locally

```bash
cargo run
```

By default, the server runs on `0.0.0.0:3001` and is accessible across your Tailnet.

## Configuration

Settings are read from a TOML file at `$TOHDOH_CONFIG`, or else
`~/.config/tohdohboard/config.toml`. Without a file there the defaults apply:
listen on `0.0.0.0:3001` and keep the board in `~/.tohdohboard/data.json`. A
file named by `TOHDOH_CONFIG` must exist; the server refuses to start rather
than fall back to the defaults. Every key is documented in
[`config.example.toml`](config.example.toml); unknown keys are an error.

```bash
TOHDOH_CONFIG=./config.toml cargo run
```

If `data.json` or `archive.json` exists but is not valid JSON, the server
answers `500` instead of serving an empty board, so no client overwrites the
damaged file. Repair or move it aside to recover. Files are written atomically (temporary file + rename).

## Archive

Finished tasks are moved off the board into `archive.json` (next to
`data.json`), which keeps the board small and every client uncluttered. When a
client loads the board (`GET /api/data`), tasks in the `done` column that were
not updated for 14 days are archived. Archived tasks are kept forever unless
`delete_after_days` is set. See the `[archive]` section of the example config.

| Route | Body | Response |
|---|---|---|
| `GET /api/archive` | | `{ "tasks": [...] }`, newest first; each task has `archivedAt` |
| `POST /api/archive` | `{ "taskIds": ["..."] }` | `{ "archived": n }` |
| `POST /api/archive/restore` | `{ "taskIds": ["..."] }` | `{ "restored": n }` |

A `PUT /api/data` that still contains an archived task drops it, so a client
with a stale copy cannot bring it back; restore it instead.

## Documentation

The source is documented with Rustdoc, including private items. Build and open the docs with:

```bash
cargo doc --no-deps --document-private-items --open
```

## Sharing with family / multiple users

With `[auth] mode = "proxy"` every user gets their own board in
`data_dir/users/<user>/`. The server does no logins itself: it trusts a reverse
proxy to say who the user is in a header (default `Tailscale-User-Login`, set
with `user_header`). Requests without the header get `401`. The value is used
as an opaque, case-sensitive id (`Alice@example.com` and `alice@example.com`
are two users) and encoded into a safe directory name (`alice@example.com`
stays readable; other characters become `%XX`). An existing single board is
not migrated: copy `data.json` into the user's directory if you want to keep it.

> **Trust boundary:** the server believes whatever `user_header` says. That is
> only safe if *every* hop in front of it is reachable from loopback only,
> except the identity proxy itself. In the setup below the web UI forwards all
> client headers to the backend, so it must listen on `127.0.0.1` too
> (`npm start` in `tohdohboard-web` does: `next start -H 127.0.0.1`). A web UI
> on `0.0.0.0:3000` would let anyone on the network pick their user.

The simplest proxy is `tailscale serve` on the machine running the server:

```toml
# ~/.config/tohdohboard/config.toml
bind = "127.0.0.1:3001"   # required in proxy mode, see below
[auth]
mode = "proxy"
```

```bash
# Web UI on 127.0.0.1:3000 (forwards /api with all headers to the backend):
(cd tohdohboard-web && npm start)
tailscale serve --bg --https=443 http://127.0.0.1:3000
# Or the API alone, e.g. for the CLI:
tailscale serve --bg --https=3001 http://127.0.0.1:3001
```

Facts worth knowing:

- `tailscale serve` adds `Tailscale-User-Login`, `Tailscale-User-Name` and
  `Tailscale-User-Profile-Pic` to requests from your tailnet, including from
  users the machine is shared with, and deletes copies sent by clients
  (documented in Tailscale's Serve docs; verified in source,
  `addTailscaleIdentityHeaders` in
  [`ipn/ipnlocal/serve.go`](https://github.com/tailscale/tailscale/blob/aac0e23d1d183ef596c446c8d1c404c57b11be30/ipn/ipnlocal/serve.go)).
  The headers are **not** set for requests from tagged devices or for Funnel
  (public) traffic, so those get `401`.
- The header is only trustworthy if every request passes through the proxy.
  The server therefore refuses to start in proxy mode unless `bind` is a
  loopback address. `allow_non_loopback = true` overrides this for container
  networks where only the proxy can reach the port. Any local process on the
  machine can still reach `127.0.0.1` and claim to be anyone.
- Tailscale's free Personal plan allows 6 users. Alternatively, *share* just
  this machine with people on other tailnets (free on all plans); they then
  appear with their own login.
- Tailscale *Services* (`svc:` names) need a tag-based host, admin approval
  and policy changes, and tagged hosts do not get user identity headers the
  same way. Their benefit, a stable name across several hosts, does not matter
  for one machine: use plain `tailscale serve` on the host.
- Other identity-aware proxies (oauth2-proxy, Authelia, Cloudflare Access...)
  work too: point `user_header` at the header they set, and make sure they
  strip it from incoming requests. Headers that clients or the HTTP layer set
  themselves (`Authorization`, `Cookie`, `Host`, `Content-Length`, ...) are
  rejected as `user_header`.

### Docker

A container is optional packaging. The image runs as an unprivileged user,
keeps data in the `/data` volume and reads `/etc/tohdohboard/config.toml`
(override with your own file or `TOHDOH_CONFIG`). The simplest setup keeps
tailscaled on the host and publishes the port on localhost only:

```bash
docker build -t tohdohboard-server .
docker run -d --name tohdoh -p 127.0.0.1:3001:3001 -v tohdoh-data:/data \
  -v ./config.toml:/etc/tohdohboard/config.toml:ro tohdohboard-server
```

Inside the container the server must bind `0.0.0.0:3001`, so a proxy-mode
config there needs `allow_non_loopback = true`; publishing on `127.0.0.1` keeps
the port away from the network. The alternative is a Tailscale sidecar
container sharing the server's network namespace. `GET /healthz` answers `ok`
without authentication for health checks.

## Security & Firewall Recommendations

In the default mode TohdohBoard has no authentication. It is **highly recommended** to only run it within a trusted network like **Tailscale**, or to use proxy mode (above).

To block public access while allowing Tailnet traffic (using `ufw`):
```bash
# Deny public access to API
sudo ufw deny 3001
# Allow access specifically from the Tailscale interface
sudo ufw allow in on tailscale0 to any port 3001
```

## Persistent Setup (systemd)

For a stable and secure production setup, it is recommended to run the server as a `systemd` service.

### 1. Build the Binary
First, compile the production-ready binary:
```bash
cd tohdohboard-backend
cargo build --release
```

### 2. Configure the Service
Create a service file at `/etc/systemd/system/tohdohboard-server.service`:

```ini
[Unit]
Description=TohdohBoard Backend Server
After=network.target

[Service]
Type=simple
User=your-username
# Replace with the actual absolute path to your backend directory
WorkingDirectory=/home/your-username/Applications/tohdohboard-backend
# Point directly to the binary to avoid build overhead or permission issues
ExecStart=/home/your-username/Applications/tohdohboard-backend/target/release/tohdohboard-server
Restart=always

# Security Hardening
ProtectSystem=full
ProtectHome=read-only
ReadWritePaths=/home/your-username/.tohdohboard
PrivateTmp=true
NoNewPrivileges=true

[Install]
WantedBy=multi-user.target
```

### 3. Permissions & Startup
Ensure your user owns the data directory:
```bash
sudo chown -R your-username:your-username ~/.tohdohboard
```

Enable and start the service:
```bash
sudo systemctl daemon-reload
sudo systemctl enable tohdohboard-server
sudo systemctl start tohdohboard-server
```

*Note: If you update the code via git pull, you must rebuild with `cargo build --release` and then run `sudo systemctl restart tohdohboard-server`.*
