# Universal Nix Installation Guide for SIAR

This guide documents the universal Nix installer for SIAR, designed to install, configure, and verify the Nix package manager across any Linux distribution and macOS with out-of-the-box **Nix Flakes** and **hermetic SIAR development environment** support.

---

## 1. Quick Start

### Option A: Local Installation (Recommended)
From the root of the cloned SIAR repository, run:

```bash
./install-nix.sh
```

Or execute directly from the `scripts/` directory:

```bash
./scripts/install-nix.sh
```

### Option B: Remote One-Liner (Pre-Clone)
If you have not yet cloned the repository, you can install Nix on any machine via `curl`:

```bash
curl --proto '=https' --tlsv1.2 -sSfL https://raw.githubusercontent.com/irshadlab/siar/develop/scripts/install-nix.sh | bash
```

### Option C: Non-Interactive / CI Installation
For headless servers, automated scripts, or CI pipelines:

```bash
./install-nix.sh --yes
```

---

## 2. Supported Distributions and Operating Systems

The installer automatically detects the host distribution, CPU architecture, and init system:

| Distribution Family | Supported Distributions | Package Manager | Init System | Notes |
| :--- | :--- | :--- | :--- | :--- |
| **Arch Linux** | Arch Linux, Manjaro, EndeavourOS, Garuda | `pacman` | `systemd` | Multi-user daemon, flakes auto-configured |
| **Debian / Ubuntu** | Ubuntu (18.04 - 24.04+), Debian, Linux Mint, Pop!_OS | `apt` | `systemd` | Automatic AppArmor user namespace compatibility |
| **RHEL / Fedora** | Fedora (38+), RHEL (8, 9), Rocky, AlmaLinux, CentOS | `dnf` / `yum` | `systemd` | Automatic SELinux labeling and policy compatibility |
| **openSUSE** | openSUSE Tumbleweed, Leap, SLES | `zypper` | `systemd` | Full multi-user daemon integration |
| **Alpine Linux** | Alpine Linux (3.16+) | `apk` | `openrc` | Supports non-systemd init and static toolchains |
| **Independent** | Void Linux (`xbps`), Gentoo (`emerge`), generic Linux | `xbps` / `portage` | `runit` / `systemd` | Fallback engine support |
| **WSL 2** | Ubuntu, Debian, or Arch on Windows Subsystem for Linux | Native | `systemd` / none | Supported with or without systemd enabled |
| **macOS** | macOS Monterey, Ventura, Sonoma, Sequoia (Intel & Apple Silicon) | Native | `launchd` | Hermetic Darwin SDK integration |

### Supported CPU Architectures
- `x86_64` (amd64)
- `aarch64` (ARM64, including Raspberry Pi 4/5, AWS Graviton, Apple Silicon)
- `armv7l` (32-bit ARM)

---

## 3. Installation Engines

The installer supports multiple underlying engines via the `--engine` parameter:

### 1. `determinate` (Default & Recommended)
Uses the [Determinate Systems Nix Installer](https://github.com/DeterminateSystems/nix-installer).
- **Why Default?**
  - Modern, high-performance Rust implementation.
  - Automatically enables modern **Nix Flakes** and `nix-command`.
  - Handles distribution-specific security quirks out-of-the-box (Ubuntu 24.04 unprivileged user namespace AppArmor policies, Fedora/RHEL SELinux contexts).
  - Creates a cryptographic receipt for safe, atomic, 100% clean uninstallation (`/nix/nix-installer uninstall`).

```bash
./install-nix.sh --engine determinate
```

### 2. `official`
Uses the standard upstream NixOS multi-user installer (`https://nixos.org/nix/install`).
- Installs the official multi-user daemon with `nixbld` users.
- Our script automatically provisions `/etc/nix/nix.conf` afterwards to enable Flakes, configure trusted users, and add binary caches.

```bash
./install-nix.sh --engine official
```

### 3. `distro`
Uses the host distribution's native package manager where available (e.g. `pacman -S nix` on Arch Linux or `apk add nix` on Alpine Linux), configures `systemd` or `openrc` service units, and tunes `/etc/nix/nix.conf`.

```bash
./install-nix.sh --engine distro
```

---

## 4. CLI Reference & Command Flags

```
SIAR Universal Nix Installer v1.0.0

Usage: install-nix.sh [OPTIONS]

Options:
  -y, --yes              Non-interactive mode (automatic yes to all prompts)
  --engine <name>        Installation engine:
                           determinate (default, recommended)
                           official    (standard upstream NixOS multi-user installer)
                           distro      (native package manager: pacman, apk)
  --multi-user           Force multi-user daemon installation (default when sudo/root available)
  --single-user          Force single-user / rootless installation (for restricted containers)
  --enable-flakes        Only configure Flakes and nix-command on an existing Nix installation
  --status               Check current Nix installation status, daemon health, and config
  --doctor               Run comprehensive verification diagnostics on Nix and Flakes
  --build-siar           Build SIAR workspace binaries using Nix after installation
  --install-siar         Install SIAR CLI directly into user's active Nix profile
  --check                Run 'nix flake check' on SIAR repository
  --uninstall            Cleanly uninstall Nix and remove all associated files and services
  --dry-run              Print the commands that would be executed without running them
  -v, --verbose          Enable verbose logging
  -h, --help             Display this help message and exit
```

---

## 5. What the Installer Configures

When run, the installer performs the following configuration automatically:

### 1. Nix Flakes & Modern CLI
Writes `/etc/nix/nix.conf` with:
```conf
experimental-features = nix-command flakes
trusted-users = root @wheel @sudo <current-user>
extra-substituters = https://cache.nixos.org https://nix-community.cachix.org
extra-trusted-public-keys = cache.nixos.org-1:6NCHdD59X431o0gWypbMrAURkbJ16ZPMQFGspcDShjY= nix-community.cachix.org-1:mB9FSh9qf2dCimDSUo8Zy7bkq5CX+/rkCWyvRCYg3Fs=
max-jobs = auto
cores = 0
keep-outputs = true
keep-derivations = true
```

### 2. Multi-User Daemon & Security
- Configures 32 isolated `nixbld` build users and the `nixbld` group.
- Enables and starts `nix-daemon.socket` and `nix-daemon.service` on systemd hosts.
- Respects and configures SELinux contexts for `/nix`.

### 3. Shell Profile Hooks
Ensures the Nix environment hook is present in:
- `/etc/profile.d/nix.sh` / `/etc/profile.d/nix-daemon.sh`
- `~/.bashrc`
- `~/.zshrc`
- `~/.profile`

---

## 6. Verification and Diagnostics

### Check Current Status
To inspect the active installation, daemon state, and store size:

```bash
./install-nix.sh --status
```

Example output:
```
[INFO] Detected System Information:
  • Distribution:   Arch Linux (arch)
  • Distro Family:  arch
  • Architecture:   x86_64 (Target: x86_64-linux)
  • Init System:    systemd

==> SIAR Nix Status Check
[SUCCESS] Nix executable found: nix (Nix) 2.24.0
  • Binary Location: /nix/var/nix/profiles/default/bin/nix
  • Daemon Service:  active (running)
  • Daemon Socket:   active (listening)
  • Flakes Support:  enabled & ready
  • /nix/store Size: 1.8G
```

### Run Environment Doctor
To run 5-stage automated diagnostics (binary verification, expression evaluation, flakes support, daemon socket connectivity, and local SIAR flake integrity):

```bash
./install-nix.sh --doctor
```

---

## 7. Working with SIAR via Nix

Once Nix is installed, you can leverage SIAR's hermetic flake:

### Enter Hermetic Development Environment
Provides exact compiler versions (Rust 1.91), GTK3, WebKit2GTK, ALSA, OpenSSL, CMake, and developer tooling:

```bash
nix develop
```

### Build SIAR Binaries
Compile reproducible binaries directly into `./result/bin/`:

```bash
# Build SIAR CLI
nix build .#siar-cli

# Build SIAR Desktop Application (Linux)
nix build .#siar-desktop

# Build SIAR Emergency DTN Mesh Node
nix build .#siar-emergency-node

# Build all workspace targets
nix build .#all
```

### Install SIAR to Profile
Install the `siar` CLI directly into your personal user profile:

```bash
nix profile install .#siar-cli
```

### Run Flake Automated Validation
```bash
nix flake check
```

---

## 8. Uninstallation

To cleanly and completely remove Nix, the `/nix` store, build users, daemon services, and shell configuration hooks:

```bash
./install-nix.sh --uninstall
```

If the Determinate installer was used, it invokes `/nix/nix-installer uninstall` for an atomic rollback. If standard Nix was installed, it safely stops and removes systemd services, deletes `nixbld` users/groups, purges `/nix`, and restores clean configuration files.
