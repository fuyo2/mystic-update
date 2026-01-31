# Mystic Update

<img src="https://raw.githubusercontent.com/fuyo2/mystic-update/7567d32b6728686bb8aafe3ba26972bbb1360dff/resources/icons/hicolor/scalable/apps/mystic-update.svg" alt="Mystic Update Logo" width="128" height="128">

![Mystic Update Application](https://raw.githubusercontent.com/fuyo2/mystic-update/refs/heads/main/resources/Mystic%20Update%20Application.png)

Mystical Software Updater for Linux Distributions.

## Installation

This application is programmed in [rust](https://github.com/rust-lang/rust) using the [libcosmic](https://github.com/pop-os/libcosmic) toolkit. Please install rust on our system before continuing.

Install missing system deps
Ubuntu / Debian / Pop!_OS / Mint:
```
sudo apt update
sudo apt install -y pkg-config libxkbcommon-dev just
```

Fedora:
```
sudo dnf install -y glib2-devel pkgconf-pkg-config libxkbcommon-devel gcc make flatpak-devel just
```

Arch / Manjaro:
```
sudo pacman -S --needed pkgconf libxkbcommon
```

openSUSE:
```
sudo zypper install -y pkg-config libxkbcommon-devel
```

A [justfile](./justfile) is included by default for the [just](https://github.com/casey/just) command runner.

- `just` builds the application with the default `just build-release` recipe
- `just run` builds and runs the application
- `just install` installs the project into the system
- `just vendor` creates a vendored tarball
- `just build-vendored` compiles with vendored dependencies from that tarball
- `just check` runs clippy on the project to check for linter warnings
- `just check-json` can be used by IDEs that support LSP

## Features

Includes the following features:
- Easy updates to your Linux system using the following package managers:
    - Apt
    - Dnf/Yum
    - Flatpak
    - Brew      *** Not Implemented Yet ***
    - Emerge    *** Not Implemented Yet ***
    - Nix-env   *** Not Implemented Yet ***
    - Pacman    *** Not Implemented Yet ***
    - Snap      *** Not Implemented Yet ***
    - Zypper    *** Not Implemented Yet ***
- Capability to install individually selected updates
- Asks if you want to reboot after updates
- View update logs from within Mystic Update application
