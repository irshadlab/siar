//! §198 DTN / emergency scenario, at the policy layer.
//!
//! The DTN *transport* (bundles, custody, store-carry-forward) lives in
//! `siar-dtn-bundle` and is not exercised here. What this crate owns, and
//! what this test walks through as one timeline, is every *decision* the
//! scenario depends on: what is fetched first, what waits, what may ride
//! a bundle, how far concurrency drops, and that resuming never restarts.

use siar_blob_manifest::app_policy::{
    plan_fetch, AutoAction, AutoDownloadPolicy, DataSaver, DerivativeLink, FetchStep, NetworkKind,
};
use siar_blob_manifest::ids::BlobId;
use siar_blob_manifest::parallelism::{effective_parallelism, ResourceContext};
use siar_blob_manifest::scheduling::{
    plan_path_change, FairScheduler, PathChangeStep, PathKind, RoutingInputs, SchedulerConfig,
    SchedulerEntry, TransferClass,
};
use siar_blob_manifest::schema_rows::DerivativeKind;
use siar_domain::{DeviceId, MediaType};

fn inputs(bulk: bool, bytes: u64) -> RoutingInputs {
    RoutingInputs {
        estimated_bytes: bytes,
        bulk,
        deadline_millis: None,
        metered_allowed: true,
        dtn_allowed: true,
        multipath_allowed: false,
    }
}

#[test]
fn an_emergency_photo_over_a_failing_mesh_is_carried_small_first_and_the_original_waits() {
    let original = BlobId::from_ciphertext(b"original photo, 8 MB");
    let thumb = BlobId::from_ciphertext(b"thumbnail, 20 KB");
    let link = DerivativeLink {
        derivative: thumb,
        kind: DerivativeKind::Thumbnail,
    };

    // 1. Emergency mode with data saver on: only the thumbnail is fetched;
    //    the original is left for later.
    let saver = DataSaver {
        enabled: true,
        pause_background_above_bytes: 1 << 20,
    };
    assert_eq!(
        plan_fetch(original, Some(link), &saver),
        vec![FetchStep::Fetch(thumb), FetchStep::DeferOriginal(original)]
    );
    // …and the thumbnail is fetched automatically even on a metered link.
    assert_eq!(
        AutoDownloadPolicy::default().decide(
            Some(MediaType::ImageJpeg),
            20_000,
            NetworkKind::Metered,
            true
        ),
        AutoAction::Download
    );

    // 2. The Wi-Fi path under the transfers disappears.
    let thumb_plan = plan_path_change(
        PathKind::WifiDirect,
        None,
        TransferClass::EmergencyCritical,
        &inputs(false, 20_000),
        64 * 1024,
        vec![(0, 1)],
    );
    // The small critical thumbnail is handed to DTN…
    assert_eq!(
        thumb_plan.steps,
        vec![PathChangeStep::PauseScheduler, PathChangeStep::HandToDtn]
    );
    // …the 8 MB original is not: bulk waits for a real path.
    let orig_plan = plan_path_change(
        PathKind::WifiDirect,
        None,
        TransferClass::Bulk,
        &inputs(true, 8_000_000),
        64 * 1024,
        vec![(0, 32)],
    );
    assert_eq!(
        orig_plan.steps,
        vec![PathChangeStep::PauseScheduler, PathChangeStep::HoldBulk]
    );

    // 3. Over DTN there is one stream, however good the device looks.
    assert_eq!(
        effective_parallelism(PathKind::Dtn, ResourceContext::default(), 16),
        1
    );

    // 4. A LAN reappears: the original resumes with exactly the missing
    //    ranges, under the same transfer — never restarted.
    let resume = plan_path_change(
        PathKind::Dtn,
        Some(PathKind::Lan),
        TransferClass::Bulk,
        &inputs(true, 8_000_000),
        64 * 1024,
        vec![(5, 32)],
    );
    assert!(resume.upgrade);
    assert!(matches!(
        resume.steps.last(),
        Some(PathChangeStep::ResumeMissing { ranges }) if ranges == &vec![(5, 32)]
    ));
}

#[test]
fn while_the_emergency_thumbnail_moves_the_bulk_original_does_not_compete_with_it() {
    let peer = DeviceId::new();
    let mut s = FairScheduler::new(SchedulerConfig::default());
    let mk = |class, chunks, bytes| SchedulerEntry {
        transfer: siar_blob_manifest::ids::TransferId::new(),
        peer,
        app: siar_blob_manifest::scheduling::AppNamespace(0),
        class,
        remaining_chunks: chunks,
        next_chunk_bytes: bytes,
    };
    let sos = mk(TransferClass::EmergencyCritical, 2, 10_000);
    let bulk = mk(TransferClass::Bulk, 100, 1 << 20);
    let sos_id = sos.transfer;
    s.register(bulk);
    s.register(sos);
    // The first two grants both go to the emergency transfer, whatever
    // order things were registered in.
    for _ in 0..2 {
        let g = s.next_grant().unwrap();
        assert_eq!(g.transfer, sos_id);
        s.complete(g);
    }
    assert_ne!(
        s.next_grant().unwrap().transfer,
        sos_id,
        "then bulk gets its turn"
    );
}
