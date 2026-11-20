#!/usr/bin/env python3
"""
Generate a unified static APT (Debian/Ubuntu) and YUM/DNF (Fedora/RHEL) repository
suitable for hosting directly on GitHub Pages (https://irshadlab.github.io/siar/).
"""

import os
import sys
import shutil
import hashlib
import gzip
import subprocess
import time
from pathlib import Path
from email.utils import formatdate

ROOT = Path(__file__).resolve().parent.parent
DIST = ROOT / "dist"
OUT = DIST / "gh-pages"
GPG_KEY_ID = "E1B2EA019D2C76FADBFFCB3E271ADFC244075A0E"

DEB_NAME = "siar_0.1.0-alpha.2_amd64.deb"
RPM_NAME = "siar-0.1.0-alpha.2-1.x86_64.rpm"

def file_hashes(path: Path):
    h_md5 = hashlib.md5()
    h_sha1 = hashlib.sha1()
    h_sha256 = hashlib.sha256()
    size = 0
    with open(path, "rb") as f:
        while chunk := f.read(65536):
            h_md5.update(chunk)
            h_sha1.update(chunk)
            h_sha256.update(chunk)
            size += len(chunk)
    return {
        "md5": h_md5.hexdigest(),
        "sha1": h_sha1.hexdigest(),
        "sha256": h_sha256.hexdigest(),
        "size": size,
    }

def build_apt_repo():
    print("[*] Building Debian/Ubuntu APT repository...")
    deb_src = DIST / DEB_NAME
    if not deb_src.exists():
        sys.exit(f"Error: {deb_src} not found")

    pool_dir = OUT / "pool" / "main" / "s" / "siar"
    pool_dir.mkdir(parents=True, exist_ok=True)
    deb_dst = pool_dir / DEB_NAME
    shutil.copy2(deb_src, deb_dst)

    # Extract control file from .deb
    cmd = f"ar p {deb_src} control.tar.xz | tar -xJO ./control"
    control_text = subprocess.check_output(cmd, shell=True, text=True).strip()

    info = file_hashes(deb_dst)
    rel_path = f"pool/main/s/siar/{DEB_NAME}"

    package_stanza = (
        f"{control_text}\n"
        f"Filename: {rel_path}\n"
        f"Size: {info['size']}\n"
        f"MD5sum: {info['md5']}\n"
        f"SHA1: {info['sha1']}\n"
        f"SHA256: {info['sha256']}\n\n"
    )

    dists_dir = OUT / "dists" / "stable" / "main" / "binary-amd64"
    dists_dir.mkdir(parents=True, exist_ok=True)
    pkg_file = dists_dir / "Packages"
    with open(pkg_file, "w") as f:
        f.write(package_stanza)

    # Compress Packages.gz
    pkg_gz_file = dists_dir / "Packages.gz"
    with open(pkg_file, "rb") as f_in, gzip.open(pkg_gz_file, "wb") as f_out:
        shutil.copyfileobj(f_in, f_out)

    # Generate Release file
    release_dir = OUT / "dists" / "stable"
    pkg_info = file_hashes(pkg_file)
    pkg_gz_info = file_hashes(pkg_gz_file)

    now = formatdate(timeval=None, localtime=False, usegmt=True)
    release_content = (
        f"Origin: SIAR\n"
        f"Label: SIAR\n"
        f"Suite: stable\n"
        f"Codename: stable\n"
        f"Version: 0.1.0\n"
        f"Architectures: amd64\n"
        f"Components: main\n"
        f"Description: Official APT repository for SIAR (Survivable Identity & Autonomous Routing)\n"
        f"Date: {now}\n"
        f"MD5Sum:\n"
        f" {pkg_info['md5']} {pkg_info['size']} main/binary-amd64/Packages\n"
        f" {pkg_gz_info['md5']} {pkg_gz_info['size']} main/binary-amd64/Packages.gz\n"
        f"SHA1:\n"
        f" {pkg_info['sha1']} {pkg_info['size']} main/binary-amd64/Packages\n"
        f" {pkg_gz_info['sha1']} {pkg_gz_info['size']} main/binary-amd64/Packages.gz\n"
        f"SHA256:\n"
        f" {pkg_info['sha256']} {pkg_info['size']} main/binary-amd64/Packages\n"
        f" {pkg_gz_info['sha256']} {pkg_gz_info['size']} main/binary-amd64/Packages.gz\n"
    )

    rel_file = release_dir / "Release"
    with open(rel_file, "w") as f:
        f.write(release_content)

    # Sign Release using GPG
    inrelease_file = release_dir / "InRelease"
    rel_gpg_file = release_dir / "Release.gpg"

    subprocess.check_call([
        "gpg", "--batch", "--yes", "--clearsign",
        "--default-key", GPG_KEY_ID,
        "-o", str(inrelease_file), str(rel_file)
    ])

    subprocess.check_call([
        "gpg", "--batch", "--yes", "-abs",
        "--default-key", GPG_KEY_ID,
        "-o", str(rel_gpg_file), str(rel_file)
    ])
    print("  [✓] APT repository built and signed.")


def build_rpm_repo():
    print("[*] Building Fedora/RHEL/openSUSE RPM repository...")
    rpm_src = DIST / RPM_NAME
    if not rpm_src.exists():
        sys.exit(f"Error: {rpm_src} not found")

    rpm_dir = OUT / "rpm"
    rpm_dir.mkdir(parents=True, exist_ok=True)
    rpm_dst = rpm_dir / RPM_NAME
    shutil.copy2(rpm_src, rpm_dst)

    # Generate siar.repo
    repo_cfg = (
        "[siar]\n"
        "name=SIAR Official Repository (Fedora / RHEL / openSUSE)\n"
        "baseurl=https://irshadlab.github.io/siar/rpm/\n"
        "enabled=1\n"
        "gpgcheck=1\n"
        "repo_gpgcheck=1\n"
        "gpgkey=https://irshadlab.github.io/siar/KEY.gpg\n"
    )
    with open(rpm_dir / "siar.repo", "w") as f:
        f.write(repo_cfg)

    repodata_dir = rpm_dir / "repodata"
    repodata_dir.mkdir(parents=True, exist_ok=True)

    info = file_hashes(rpm_dst)
    now_ts = int(time.time())

    primary_xml = f"""<?xml version="1.0" encoding="UTF-8"?>
<metadata xmlns="http://linux.duke.edu/metadata/common" xmlns:rpm="http://linux.duke.edu/metadata/rpm" packages="1">
  <package type="rpm">
    <name>siar</name>
    <arch>x86_64</arch>
    <version epoch="0" ver="0.1.0_alpha.2" rel="1"/>
    <checksum type="sha256" pkgid="YES">{info['sha256']}</checksum>
    <summary>Survivable Identity &amp; Autonomous Routing: decentralized messaging</summary>
    <description>Survivable Identity &amp; Autonomous Routing (SIAR): Local-first, zero-infrastructure, multi-transport, offline-first decentralized messaging and delay-tolerant networking platform.</description>
    <packager>Irshad Ali &lt;irshadali5@proton.me&gt;</packager>
    <url>https://github.com/irshadlab/siar</url>
    <time file="{now_ts}" build="{now_ts}"/>
    <size package="{info['size']}" installed="55595000" archive="55600000"/>
    <location href="{RPM_NAME}"/>
    <format>
      <rpm:license>AGPL-3.0-or-later</rpm:license>
      <rpm:vendor>Irshad Lab</rpm:vendor>
      <rpm:group>Applications/Internet</rpm:group>
      <rpm:buildhost>archlinux</rpm:buildhost>
      <rpm:sourcerpm>siar-0.1.0_alpha.2-1.src.rpm</rpm:sourcerpm>
      <rpm:header-range start="280" end="4096"/>
      <rpm:provides>
        <rpm:entry name="siar" flags="EQ" epoch="0" ver="0.1.0_alpha.2" rel="1"/>
        <rpm:entry name="siar-daemon"/>
        <rpm:entry name="siar-desktop"/>
      </rpm:provides>
      <rpm:requires>
        <rpm:entry name="gtk3"/>
        <rpm:entry name="alsa-lib"/>
        <rpm:entry name="openssl"/>
      </rpm:requires>
      <file>/usr/bin/siar</file>
      <file>/usr/bin/siar-daemon</file>
      <file>/usr/bin/siar-desktop</file>
    </format>
  </package>
</metadata>
"""
    primary_gz = repodata_dir / "primary.xml.gz"
    with gzip.open(primary_gz, "wb") as f_out:
        f_out.write(primary_xml.encode("utf-8"))

    gz_info = file_hashes(primary_gz)
    uncomp_sha256 = hashlib.sha256(primary_xml.encode("utf-8")).hexdigest()
    uncomp_size = len(primary_xml.encode("utf-8"))

    # Rename to standard repodata hash prefix
    hashed_primary_name = f"{gz_info['sha256']}-primary.xml.gz"
    hashed_primary_path = repodata_dir / hashed_primary_name
    primary_gz.rename(hashed_primary_path)

    repomd_xml = f"""<?xml version="1.0" encoding="UTF-8"?>
<repomd xmlns="http://linux.duke.edu/metadata/repo">
  <revision>{now_ts}</revision>
  <data type="primary">
    <checksum type="sha256">{gz_info['sha256']}</checksum>
    <open-checksum type="sha256">{uncomp_sha256}</open-checksum>
    <location href="repodata/{hashed_primary_name}"/>
    <timestamp>{now_ts}</timestamp>
    <size>{gz_info['size']}</size>
    <open-size>{uncomp_size}</open-size>
  </data>
</repomd>
"""
    repomd_file = repodata_dir / "repomd.xml"
    with open(repomd_file, "w") as f:
        f.write(repomd_xml)

    # Sign repomd.xml
    repomd_asc = repodata_dir / "repomd.xml.asc"
    subprocess.check_call([
        "gpg", "--batch", "--yes", "-abs",
        "--default-key", GPG_KEY_ID,
        "-o", str(repomd_asc), str(repomd_file)
    ])
    print("  [✓] RPM repository built and signed.")


def build_landing_page():
    print("[*] Generating landing page and GPG public keys...")
    # Export public GPG key
    key_pub = subprocess.check_output([
        "gpg", "--armor", "--export", GPG_KEY_ID
    ], text=True)

    with open(OUT / "KEY.gpg", "w") as f:
        f.write(key_pub)
    with open(OUT / "rpm" / "RPM-GPG-KEY-siar", "w") as f:
        f.write(key_pub)

    index_html = """<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>SIAR Package Repositories</title>
  <style>
    :root {
      --bg: #0d1117;
      --card-bg: #161b22;
      --border: #30363d;
      --text: #c9d1d9;
      --heading: #f0f6fc;
      --accent: #58a6ff;
      --code-bg: #090d13;
    }
    body {
      font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Helvetica, Arial, sans-serif;
      background: var(--bg);
      color: var(--text);
      line-height: 1.6;
      max-width: 900px;
      margin: 0 auto;
      padding: 40px 20px;
    }
    h1, h2, h3 { color: var(--heading); }
    a { color: var(--accent); text-decoration: none; }
    a:hover { text-decoration: underline; }
    .badge {
      display: inline-block;
      padding: 4px 10px;
      border-radius: 20px;
      font-size: 0.85em;
      font-weight: 600;
      background: #1f6feb33;
      color: #58a6ff;
      margin-bottom: 20px;
    }
    .grid {
      display: grid;
      grid-template-columns: 1fr;
      gap: 20px;
      margin-top: 30px;
    }
    @media (min-width: 768px) {
      .grid { grid-template-columns: 1fr 1fr; }
    }
    .card {
      background: var(--card-bg);
      border: 1px solid var(--border);
      border-radius: 8px;
      padding: 24px;
    }
    pre {
      background: var(--code-bg);
      border: 1px solid var(--border);
      border-radius: 6px;
      padding: 14px;
      overflow-x: auto;
      font-size: 0.9em;
      color: #79c0ff;
    }
    code { font-family: ui-monospace, SFMono-Regular, SF Mono, Menlo, Consolas, monospace; }
  </style>
</head>
<body>
  <span class="badge">Official Distribution</span>
  <h1>SIAR Package Repositories</h1>
  <p>Install and automatically receive updates for <strong>SIAR (Survivable Identity & Autonomous Routing)</strong> across Linux distributions and Windows.</p>

  <div class="grid">
    <div class="card">
      <h3>🐧 Ubuntu &amp; Debian (APT)</h3>
      <p>Add the official APT repository to receive automated updates:</p>
      <pre><code># 1. Download official GPG key
sudo install -m 0755 -d /etc/apt/keyrings
curl -fsSL https://irshadlab.github.io/siar/KEY.gpg | sudo gpg --dearmor --yes -o /etc/apt/keyrings/siar.gpg
sudo chmod a+r /etc/apt/keyrings/siar.gpg

# 2. Add repository source
echo "deb [signed-by=/etc/apt/keyrings/siar.gpg] https://irshadlab.github.io/siar stable main" | sudo tee /etc/apt/sources.list.d/siar.list

# 3. Update &amp; Install SIAR
sudo apt update
sudo apt install siar</code></pre>
    </div>

    <div class="card">
      <h3>🎩 Fedora &amp; Red Hat (DNF / RPM)</h3>
      <p>Add the official RPM repository to receive automated updates:</p>
      <pre><code># 1. Add repository configuration
sudo curl -fsSL https://irshadlab.github.io/siar/rpm/siar.repo -o /etc/yum.repos.d/siar.repo

# 2. Install SIAR
sudo dnf install siar</code></pre>
    </div>

    <div class="card">
      <h3>🏹 Arch Linux (AUR)</h3>
      <p>Install via your preferred AUR helper:</p>
      <pre><code># Precompiled binary (Recommended)
paru -S siar-bin

# Or build from source
paru -S siar-git</code></pre>
    </div>

    <div class="card">
      <h3>🪟 Windows (winget / ZIP)</h3>
      <p>Install via Windows Package Manager or GitHub release:</p>
      <pre><code># Direct ZIP Download
Available on GitHub Releases

# Or extract and run:
.\\siar-desktop.exe</code></pre>
    </div>
  </div>

  <footer style="margin-top: 50px; text-align: center; border-top: 1px solid var(--border); padding-top: 20px; font-size: 0.9em;">
    <p>Maintained by <a href="https://github.com/irshadlab">Irshad Lab</a> • <a href="https://github.com/irshadlab/siar">GitHub Source Repository</a></p>
  </footer>
</body>
</html>
"""
    with open(OUT / "index.html", "w") as f:
        f.write(index_html)

    # Add a .nojekyll file so GitHub Pages doesn't ignore directories starting with underscores
    with open(OUT / ".nojekyll", "w") as f:
        f.write("")

    print("  [✓] Landing page and .nojekyll created.")

def main():
    OUT.mkdir(parents=True, exist_ok=True)
    build_apt_repo()
    build_rpm_repo()
    build_landing_page()
    print(f"\n[🎉] Complete repository tree built successfully in: {OUT}")

if __name__ == "__main__":
    main()
