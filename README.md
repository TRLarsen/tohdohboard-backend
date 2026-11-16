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

## Security & Firewall Recommendations

TohdohBoard does not have built-in authentication. It is **highly recommended** to only run it within a trusted network like **Tailscale**.

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
