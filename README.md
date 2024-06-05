# Tango Bridge

Tango Bridge is a system-tray application that can start ADB and forward connections to Tango Web App running in a browser.

An ADB executable for each platform is included.

## Build

### Prerequisites

- Rust

### Windows

Visual Studio is required to build the project.

```sh
./build.ps1
```

### macOS

1. Open `build.sh`
2. Change `sdk_version` to the version of MacOS SDK installed on your system

```sh
./build.sh
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
