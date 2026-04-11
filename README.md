# TohdohBoard Server

A local REST API backend for the TohdohBoard ecosystem, built with Rust and Axum.

## Overview
This server allows the `tohdohboard-web` and `tohdohboard-cli` clients to sync state using a single source of truth. It reads and writes the shared `schema.json` compliant data to the local disk.

## Running

```bash
cargo run
```

By default, the server runs on `http://127.0.0.1:3001` and serves the data file.
