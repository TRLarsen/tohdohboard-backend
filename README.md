# TohdohBoard Backend (Server)

A local REST API backend for the TohdohBoard ecosystem, built with Rust and Axum.

## Overview
This server allows the `tohdohboard-web` and `tohdohboard-cli` clients to sync state using a single source of truth. It reads and writes the shared `schema.json` compliant data to the local disk.

## Running Locally

```bash
cargo run
```

By default, the server runs on `0.0.0.0:3001` and is accessible across your Tailnet.

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

To keep the server running continuously and securely, create a service file at `/etc/systemd/system/tohdohboard-server.service`:

```ini
[Unit]
Description=TohdohBoard Backend Server
After=network.target

[Service]
Type=simple
User=your-username
# Replace with the actual absolute path to your backend directory
WorkingDirectory=/home/your-username/tohdohboard-backend
ExecStart=/usr/bin/cargo run --release
Restart=always

# Security Hardening
# Limits the "blast radius" by restricting filesystem access
ProtectSystem=strict
ProtectHome=read-only
ReadWritePaths=/home/your-username/.tohdohboard
PrivateTmp=true
NoNewPrivileges=true

[Install]
WantedBy=multi-user.target
```

Then run:
```bash
sudo systemctl daemon-reload
sudo systemctl enable tohdohboard-server
sudo systemctl start tohdohboard-server
```
