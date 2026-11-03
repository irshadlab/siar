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
    name: &'static str,
    device_id: DeviceId,
    identity: DeviceIdentity,
    cert: DeviceCertificate,
    subpath: &'static str, // e.g. "alice-cli/siar-cli"
}

fn write_device_files(target_dir: &Path, device: &ProvisionedDevice, account_id: &AccountId) -> Result<()> {
    let dir = target_dir.join(device.subpath);
    std::fs::create_dir_all(&dir)
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

    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let out_dir = if args.len() > 1 {
        PathBuf::from(&args[1])
    } else {
        PathBuf::from("/tmp/siar-k8s-data")
    };

    println!("============================================================");
    println!("SIAR MULTI-USER MULTI-DEVICE PROVISIONING");
    println!("Target data root: {}", out_dir.display());
    println!("============================================================\n");

    let now = now_millis();

    // -------------------------------------------------------------
    // USER 1: ALICE
    // -------------------------------------------------------------
    let alice_account_id = AccountId::from_uuid(
        uuid::Uuid::parse_str("018d4510-b98a-7788-99aa-111111111111").unwrap(),
    );
    let alice_root_key = RootIdentityKey::generate();
    let alice_root_pub = alice_root_key.root_public_key();

    println!("▶ USER 1: Alice");
    println!("  Account ID:        {}", alice_account_id);
    println!("  Root Public Key:   {}", hex_encode(&alice_root_pub.0));

    // Alice Device 1: CLI
    let alice_cli_dev_id = DeviceId::from_uuid(
        uuid::Uuid::parse_str("018d4510-b98a-7788-99aa-111111111101").unwrap(),
    );
    let alice_cli_ident = DeviceIdentity::generate();
    let alice_cli_pk = *alice_cli_ident.verifying_key().as_bytes();
    let alice_cli_cert = DeviceCertificate::issue(
        &alice_root_key,
        alice_account_id,
        alice_cli_dev_id,
        alice_cli_pk,
        now,
        None,
        DeviceCapabilitySet::ALL,
        1,
    );
    alice_cli_cert
        .verify_signature(&alice_root_pub)
        .expect("Alice CLI cert signature invalid");

    // Alice Device 2: Daemon
    let alice_daemon_dev_id = DeviceId::from_uuid(
        uuid::Uuid::parse_str("018d4510-b98a-7788-99aa-111111111102").unwrap(),
    );
    let alice_daemon_ident = DeviceIdentity::generate();
    let alice_daemon_pk = *alice_daemon_ident.verifying_key().as_bytes();
    let alice_daemon_cert = DeviceCertificate::issue(
        &alice_root_key,
        alice_account_id,
        alice_daemon_dev_id,
        alice_daemon_pk,
        now,
        None,
        DeviceCapabilitySet::ALL,
        1,
    );
    alice_daemon_cert
        .verify_signature(&alice_root_pub)
        .expect("Alice Daemon cert signature invalid");

    // Alice Device 3: Desktop UI
    let alice_desktop_dev_id = DeviceId::from_uuid(
        uuid::Uuid::parse_str("018d4510-b98a-7788-99aa-111111111103").unwrap(),
    );
    let alice_desktop_ident = DeviceIdentity::generate();
    let alice_desktop_pk = *alice_desktop_ident.verifying_key().as_bytes();
    let alice_desktop_cert = DeviceCertificate::issue(
        &alice_root_key,
        alice_account_id,
        alice_desktop_dev_id,
        alice_desktop_pk,
        now,
        None,
        DeviceCapabilitySet::ALL,
        1,
    );
    alice_desktop_cert
        .verify_signature(&alice_root_pub)
        .expect("Alice Desktop cert signature invalid");

    // Alice's Signed Device Directory (Generation 1, 3 active devices)
    let alice_entries = vec![
        DeviceDirectoryEntry {
            device_id: alice_cli_dev_id,
            certificate: alice_cli_cert.clone(),
            status: DeviceStatus::Active,
            transport_endpoints: Vec::new(),
        },
        DeviceDirectoryEntry {
            device_id: alice_daemon_dev_id,
            certificate: alice_daemon_cert.clone(),
            status: DeviceStatus::Active,
            transport_endpoints: Vec::new(),
        },
        DeviceDirectoryEntry {
            device_id: alice_desktop_dev_id,
            certificate: alice_desktop_cert.clone(),
            status: DeviceStatus::Active,
            transport_endpoints: Vec::new(),
        },
    ];

    let alice_directory = DeviceDirectory::sign(&alice_root_key, alice_account_id, 1, alice_entries);
    alice_directory
        .verify_signature(&alice_root_pub)
        .expect("Alice directory signature verification failed");

    println!("  Registered Devices in Alice's Account:");
    println!("    [1] Alice CLI:     Device ID: {} (Ed25519: {})", alice_cli_dev_id, hex_encode(&alice_cli_pk));
    println!("    [2] Alice Daemon:  Device ID: {} (Ed25519: {})", alice_daemon_dev_id, hex_encode(&alice_daemon_pk));
    println!("    [3] Alice Desktop: Device ID: {} (Ed25519: {})", alice_desktop_dev_id, hex_encode(&alice_desktop_pk));
    println!("  Signed Device Directory: Gen {}, Active Devices Count: {}\n", alice_directory.generation, alice_directory.active_devices().count());

    // -------------------------------------------------------------
    // USER 2: BOB
    // -------------------------------------------------------------
    let bob_account_id = AccountId::from_uuid(
        uuid::Uuid::parse_str("018d4510-b98a-7788-99aa-222222222222").unwrap(),
    );
    let bob_root_key = RootIdentityKey::generate();
    let bob_root_pub = bob_root_key.root_public_key();

    println!("▶ USER 2: Bob");
    println!("  Account ID:        {}", bob_account_id);
    println!("  Root Public Key:   {}", hex_encode(&bob_root_pub.0));

    // Bob Device 1: Daemon
    let bob_daemon_dev_id = DeviceId::from_uuid(
        uuid::Uuid::parse_str("018d4510-b98a-7788-99aa-222222222201").unwrap(),
    );
    let bob_daemon_ident = DeviceIdentity::generate();
    let bob_daemon_pk = *bob_daemon_ident.verifying_key().as_bytes();
    let bob_daemon_cert = DeviceCertificate::issue(
        &bob_root_key,
        bob_account_id,
        bob_daemon_dev_id,
        bob_daemon_pk,
        now,
        None,
        DeviceCapabilitySet::ALL,
        1,
    );
    bob_daemon_cert
        .verify_signature(&bob_root_pub)
        .expect("Bob Daemon cert signature invalid");

    // Bob Device 2: CLI
    let bob_cli_dev_id = DeviceId::from_uuid(
        uuid::Uuid::parse_str("018d4510-b98a-7788-99aa-222222222202").unwrap(),
    );
    let bob_cli_ident = DeviceIdentity::generate();
    let bob_cli_pk = *bob_cli_ident.verifying_key().as_bytes();
    let bob_cli_cert = DeviceCertificate::issue(
        &bob_root_key,
        bob_account_id,
        bob_cli_dev_id,
        bob_cli_pk,
        now,
        None,
        DeviceCapabilitySet::ALL,
        1,
    );
    bob_cli_cert
        .verify_signature(&bob_root_pub)
        .expect("Bob CLI cert signature invalid");

    let bob_entries = vec![
        DeviceDirectoryEntry {
            device_id: bob_daemon_dev_id,
            certificate: bob_daemon_cert.clone(),
            status: DeviceStatus::Active,
            transport_endpoints: Vec::new(),
        },
        DeviceDirectoryEntry {
            device_id: bob_cli_dev_id,
            certificate: bob_cli_cert.clone(),
            status: DeviceStatus::Active,
            transport_endpoints: Vec::new(),
        },
    ];

    let bob_directory = DeviceDirectory::sign(&bob_root_key, bob_account_id, 1, bob_entries);
    bob_directory
        .verify_signature(&bob_root_pub)
        .expect("Bob directory signature verification failed");

    println!("  Registered Devices in Bob's Account:");
    println!("    [1] Bob Daemon:    Device ID: {} (Ed25519: {})", bob_daemon_dev_id, hex_encode(&bob_daemon_pk));
    println!("    [2] Bob CLI:       Device ID: {} (Ed25519: {})", bob_cli_dev_id, hex_encode(&bob_cli_pk));
    println!("  Signed Device Directory: Gen {}, Active Devices Count: {}\n", bob_directory.generation, bob_directory.active_devices().count());

    // -------------------------------------------------------------
    // WRITE PERSISTENCE FILES FOR RUNTIMES
    // -------------------------------------------------------------
    let alice_devices = [
        ProvisionedDevice {
            name: "Alice CLI",
            device_id: alice_cli_dev_id,
            identity: alice_cli_ident,
            cert: alice_cli_cert,
            subpath: "alice-cli/siar-cli",
        },
        ProvisionedDevice {
            name: "Alice Daemon",
            device_id: alice_daemon_dev_id,
            identity: alice_daemon_ident,
            cert: alice_daemon_cert,
            subpath: "alice-daemon/siar",
        },
        ProvisionedDevice {
            name: "Alice Desktop",
            device_id: alice_desktop_dev_id,
            identity: alice_desktop_ident,
            cert: alice_desktop_cert,
            subpath: "alice-desktop/siar",
        },
    ];

    for d in &alice_devices {
        write_device_files(&out_dir, d, &alice_account_id)?;
        println!("✓ Wrote device persistence files for {} at {}/{}", d.name, out_dir.display(), d.subpath);
    }

    let bob_devices = [
        ProvisionedDevice {
            name: "Bob Daemon",
            device_id: bob_daemon_dev_id,
            identity: bob_daemon_ident,
            cert: bob_daemon_cert,
            subpath: "bob-daemon/siar",
        },
        ProvisionedDevice {
            name: "Bob CLI",
            device_id: bob_cli_dev_id,
            identity: bob_cli_ident,
            cert: bob_cli_cert,
            subpath: "bob-cli/siar-cli",
        },
    ];

    for d in &bob_devices {
        write_device_files(&out_dir, d, &bob_account_id)?;
        println!("✓ Wrote device persistence files for {} at {}/{}", d.name, out_dir.display(), d.subpath);
    }

    // Write a JSON summary of all cryptographic identities and certificates
    let mut manifest = String::new();
    manifest.push_str("{\n");
    manifest.push_str(&format!("  \"timestamp_millis\": {},\n", now));
    manifest.push_str("  \"users\": [\n");

    // Alice JSON
    manifest.push_str("    {\n");
    manifest.push_str("      \"username\": \"Alice\",\n");
    manifest.push_str(&format!("      \"account_id\": \"{}\",\n", alice_account_id));
    manifest.push_str(&format!("      \"root_public_key\": \"{}\",\n", hex_encode(&alice_root_pub.0)));
    manifest.push_str(&format!("      \"directory_generation\": {},\n", alice_directory.generation));
    manifest.push_str(&format!("      \"directory_signature\": \"{}\",\n", hex_encode(&alice_directory.signature)));
    manifest.push_str("      \"devices\": [\n");
    for (i, d) in alice_devices.iter().enumerate() {
        let comma = if i + 1 < alice_devices.len() { "," } else { "" };
        manifest.push_str("        {\n");
        manifest.push_str(&format!("          \"name\": \"{}\",\n", d.name));
        manifest.push_str(&format!("          \"device_id\": \"{}\",\n", d.device_id));
        manifest.push_str(&format!("          \"subpath\": \"{}\",\n", d.subpath));
        manifest.push_str(&format!("          \"ed25519_verifying_key\": \"{}\",\n", hex_encode(d.identity.verifying_key().as_bytes())));
        manifest.push_str(&format!("          \"x25519_public_key\": \"{}\",\n", hex_encode(d.identity.x25519_public().as_bytes())));
        manifest.push_str("          \"certificate\": {\n");
        manifest.push_str(&format!("            \"generation\": {},\n", d.cert.generation));
        manifest.push_str(&format!("            \"capabilities_mask\": {},\n", d.cert.capabilities.0));
        manifest.push_str(&format!("            \"signature\": \"{}\"\n", hex_encode(&d.cert.signature)));
        manifest.push_str("          }\n");
        manifest.push_str(&format!("        }}{}\n", comma));
    }
    manifest.push_str("      ]\n");
    manifest.push_str("    },\n");

    // Bob JSON
    manifest.push_str("    {\n");
    manifest.push_str("      \"username\": \"Bob\",\n");
    manifest.push_str(&format!("      \"account_id\": \"{}\",\n", bob_account_id));
    manifest.push_str(&format!("      \"root_public_key\": \"{}\",\n", hex_encode(&bob_root_pub.0)));
    manifest.push_str(&format!("      \"directory_generation\": {},\n", bob_directory.generation));
    manifest.push_str(&format!("      \"directory_signature\": \"{}\",\n", hex_encode(&bob_directory.signature)));
    manifest.push_str("      \"devices\": [\n");
    for (i, d) in bob_devices.iter().enumerate() {
        let comma = if i + 1 < bob_devices.len() { "," } else { "" };
        manifest.push_str("        {\n");
        manifest.push_str(&format!("          \"name\": \"{}\",\n", d.name));
        manifest.push_str(&format!("          \"device_id\": \"{}\",\n", d.device_id));
        manifest.push_str(&format!("          \"subpath\": \"{}\",\n", d.subpath));
        manifest.push_str(&format!("          \"ed25519_verifying_key\": \"{}\",\n", hex_encode(d.identity.verifying_key().as_bytes())));
        manifest.push_str(&format!("          \"x25519_public_key\": \"{}\",\n", hex_encode(d.identity.x25519_public().as_bytes())));
        manifest.push_str("          \"certificate\": {\n");
        manifest.push_str(&format!("            \"generation\": {},\n", d.cert.generation));
        manifest.push_str(&format!("            \"capabilities_mask\": {},\n", d.cert.capabilities.0));
        manifest.push_str(&format!("            \"signature\": \"{}\"\n", hex_encode(&d.cert.signature)));
        manifest.push_str("          }\n");
        manifest.push_str(&format!("        }}{}\n", comma));
    }
    manifest.push_str("      ]\n");
    manifest.push_str("    }\n");
    manifest.push_str("  ]\n");
    manifest.push_str("}\n");

    std::fs::write(out_dir.join("provisioning_manifest.json"), manifest)?;
    println!("\n✓ Generated comprehensive manifest: {}/provisioning_manifest.json", out_dir.display());

    Ok(())
}
