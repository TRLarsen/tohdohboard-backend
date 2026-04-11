# TohdohBoard Backend (Server)

A local REST API backend for the TohdohBoard ecosystem, built with Rust and Axum.

## Overview
This server allows the `tohdohboard-web` and `tohdohboard-cli` clients to sync state using a single source of truth. It reads and writes the shared `schema.json` compliant data to the local disk.

## Running Locally

```bash
cargo run
```

By default, the server runs on `0.0.0.0:3001` and is accessible across your Tailnet.

## Persistent Setup (systemd)

To keep the server running continuously, create a systemd service file at `/etc/systemd/system/tohdohboard-server.service`:

```ini
[Unit]
Description=TohdohBoard Backend Server
After=network.target

[Service]
Type=simple
User=your-username
WorkingDirectory=/path/to/tohdohboard-backend
ExecStart=/usr/bin/cargo run --release
Restart=always

[Install]
WantedBy=multi-user.target
```

Then run:
```bash
sudo systemctl daemon-reload
sudo systemctl enable tohdohboard-server
sudo systemctl start tohdohboard-server
```
