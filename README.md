# Tango Bridge

Tango Bridge is a system-tray application that can start ADB and forward connections to Tango Web App running in a browser.

An ADB executable for each platform is included.

## Build

### Prerequisites

- [Rust](https://rustup.rs/)

### Windows

```sh
cargo build --release
```

### macOS

Xcode is required to build aarch64 (arm64) version.

```sh
cargo build --release
```

### Linux

gtk3 and libappindicator3 are required to build the project.

Arch Linux / Manjaro:

```sh
sudo pacman -S gtk3 libappindicator-gtk3
```

Ubuntu / Debian:

```sh
sudo apt install libgtk-3-dev libappindicator3-dev
```

```sh
cargo build --release
```
