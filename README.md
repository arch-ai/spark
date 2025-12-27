# Spark

A terminal task manager written in Rust. It shows live processes, supports sorting/filtering, and includes a Docker view with container stats enabling logs and exec for any docker container.

## Build (Linux)

```bash
cargo build --release
```

Binary output:

```
target/release/spark
```

## Run

```bash
cargo run
```

## Install (Linux)
Installer made for Ubuntu.

```bash
./install.sh
```

By default the install script copies the binary to `~/.local/bin/spark`.
You can override the destination prefix:

```bash
PREFIX=/opt/spark ./install.sh
```

The install script also creates a desktop entry at
`~/.local/share/applications/spark.desktop` so the app appears in the
Ubuntu launcher as "Spark". On Ubuntu it uses `gnome-terminal` with a custom
WM_CLASS so it can be pinned separately in the dash. The installer detects
Wayland via `GDK_BACKEND`, `WAYLAND_DISPLAY`, or `XDG_SESSION_TYPE` and forces
`GDK_BACKEND=x11` for the launcher entry. If you want to override this, re-run
the installer with:

```bash
FORCE_X11=1 ./install.sh
```

## Notes

- Docker view requires the `docker` CLI in `PATH`.
- Container shell uses `docker exec` and opens a new terminal window.
