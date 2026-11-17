use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use siar_crypto::DeviceIdentity;
use siar_domain::{AccountId, DeviceId};
use siar_identity_multidevice::capability::DeviceCapabilitySet;
use siar_identity_multidevice::certificate::DeviceCertificate;
use siar_identity_multidevice::directory::{DeviceDirectory, DeviceDirectoryEntry, DeviceStatus};
use siar_identity_multidevice::root_key::RootIdentityKey;

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        use std::fmt::Write;
        write!(&mut s, "{:02x}", b).unwrap();
    }
    s
}

struct ProvisionedDevice {
    name: String,
    device_id: DeviceId,
    identity: DeviceIdentity,
    cert: DeviceCertificate,
    subpath: String,
}

fn write_device_files(target_dir: &Path, device: &ProvisionedDevice, account_id: &AccountId) -> Result<()> {
    let base_dir = target_dir.join(&device.subpath);
    let subdirs = vec![
        base_dir.clone(),
        base_dir.join("siar"),
        base_dir.join("siar-cli"),
    ];

    let cert_bytes = postcard::to_allocvec(&device.cert)
        .map_err(|e| anyhow::anyhow!("serializing certificate: {:?}", e))?;

    for dir in &subdirs {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("creating directory {}", dir.display()))?;

        // 1. account_id.txt
        std::fs::write(dir.join("account_id.txt"), account_id.to_string())
            .with_context(|| format!("writing account_id.txt in {}", dir.display()))?;

        // 2. device_id.txt
        std::fs::write(dir.join("device_id.txt"), device.device_id.to_string())
            .with_context(|| format!("writing device_id.txt in {}", dir.display()))?;

        // 3. identity.bin
        device
            .identity
            .save_to_file(&dir.join("identity.bin"))
            .map_err(|e| anyhow::anyhow!("saving identity.bin: {:?}", e))?;

        // 4. certificate.bin
        std::fs::write(dir.join("certificate.bin"), &cert_bytes)
            .with_context(|| format!("writing certificate.bin in {}", dir.display()))?;
    }

    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let out_dir = if args.len() > 1 {
        PathBuf::from(&args[1])
    } else {
        PathBuf::from("/tmp/siar-500-testbed")
    };

    let user_count: usize = if args.len() > 2 {
        args[2].parse().unwrap_or(500)
    } else {
        500
    };

    println!("============================================================");
    println!("SIAR MULTI-USER MULTI-DEVICE PROVISIONING");
    println!("Total Users Target: {}", user_count);
    println!("Target data root:   {}", out_dir.display());
    println!("============================================================\n");

    let start_time = std::time::Instant::now();
    let now = now_millis();

    let mut total_devices = 0;
    let mut manifest = String::new();
    manifest.push_str("{\n");
    manifest.push_str(&format!("  \"timestamp_millis\": {},\n", now));
    manifest.push_str(&format!("  \"total_users\": {},\n", user_count));
    manifest.push_str("  \"users\": [\n");

    for u in 1..=user_count {
        let account_id = AccountId::from_uuid(uuid::Uuid::new_v4());
        let root_key = RootIdentityKey::generate();
        let root_pub = root_key.root_public_key();

        // 2 devices for all users; first 100 users have 3 devices (phone, laptop, desktop)
        let device_types = if u <= 100 {
            vec!["phone", "laptop", "desktop"]
        } else {
            vec!["phone", "laptop"]
        };

        let mut provisioned = Vec::new();
        let mut dir_entries = Vec::new();

        for d_type in &device_types {
            let dev_id = DeviceId::from_uuid(uuid::Uuid::new_v4());
            let ident = DeviceIdentity::generate();
            let pk = *ident.verifying_key().as_bytes();
            let cert = DeviceCertificate::issue(
                &root_key,
                account_id,
                dev_id,
                pk,
                now,
                None,
                DeviceCapabilitySet::ALL,
                1,
            );
            cert.verify_signature(&root_pub)
                .expect("cert signature verification failed");

            dir_entries.push(DeviceDirectoryEntry {
                device_id: dev_id,
                certificate: cert.clone(),
                status: DeviceStatus::Active,
                transport_endpoints: Vec::new(),
            });

            provisioned.push(ProvisionedDevice {
                name: format!("user-{:04}-{}", u, d_type),
                device_id: dev_id,
                identity: ident,
                cert,
                subpath: format!("user-{:04}-{}", u, d_type),
            });
        }

        let dir = DeviceDirectory::sign(&root_key, account_id, 1, dir_entries);
        dir.verify_signature(&root_pub)
            .expect("directory signature verification failed");

        for d in &provisioned {
            write_device_files(&out_dir, d, &account_id)?;
            total_devices += 1;
        }

        let shard_id = (u - 1) / 100;
        let comma_user = if u < user_count { "," } else { "" };

        manifest.push_str("    {\n");
        manifest.push_str(&format!("      \"user_index\": {},\n", u));
        manifest.push_str(&format!("      \"shard_id\": {},\n", shard_id));
        manifest.push_str(&format!("      \"account_id\": \"{}\",\n", account_id));
        manifest.push_str(&format!("      \"root_public_key\": \"{}\",\n", hex_encode(&root_pub.0)));
        manifest.push_str(&format!("      \"directory_generation\": {},\n", dir.generation));
        manifest.push_str("      \"devices\": [\n");

        for (i, d) in provisioned.iter().enumerate() {
            let comma_dev = if i + 1 < provisioned.len() { "," } else { "" };
            manifest.push_str("        {\n");
            manifest.push_str(&format!("          \"name\": \"{}\",\n", d.name));
            manifest.push_str(&format!("          \"device_id\": \"{}\",\n", d.device_id));
            manifest.push_str(&format!("          \"subpath\": \"{}\",\n", d.subpath));
            manifest.push_str(&format!("          \"ed25519_verifying_key\": \"{}\",\n", hex_encode(d.identity.verifying_key().as_bytes())));
            manifest.push_str(&format!("          \"x25519_public_key\": \"{}\"\n", hex_encode(d.identity.x25519_public().as_bytes())));
            manifest.push_str(&format!("        }}{}\n", comma_dev));
        }

        manifest.push_str("      ]\n");
        manifest.push_str(&format!("    }}{}\n", comma_user));

        if u % 100 == 0 || u == user_count {
            println!("  -> Provisioned {}/{} users (total {} devices)...", u, user_count, total_devices);
        }
    }

    manifest.push_str("  ]\n");
    manifest.push_str("}\n");

    std::fs::write(out_dir.join("provisioning_manifest.json"), manifest)?;

    let elapsed = start_time.elapsed();
    println!("\n============================================================");
    println!("✓ Provisioning Complete in {:.2?}", elapsed);
    println!("  Total Users:        {}", user_count);
    println!("  Total Devices:      {}", total_devices);
    println!("  Manifest Generated: {}/provisioning_manifest.json", out_dir.display());
    println!("============================================================");

    Ok(())
}
