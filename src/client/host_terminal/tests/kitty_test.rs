#[test]
fn graphics_update_uploads_once_then_repositions_only() {
    let mut cache = HostGraphicsCache::default();
    let first = update(&mut cache, &[test_placement(0, 0)], false);
    assert!(String::from_utf8_lossy(&first).contains("a=t"));
    assert!(String::from_utf8_lossy(&first).contains("a=p"));
    assert!(update(&mut cache, &[test_placement(0, 0)], false).is_empty());

    let mut changed = test_placement(0, 0);
    changed.placement.z = 1;
    for placement in [changed, test_placement(0, 1)] {
        let bytes = update(&mut cache, &[placement], false);
        assert!(!String::from_utf8_lossy(&bytes).contains("a=t"));
        assert!(String::from_utf8_lossy(&bytes).contains("a=p"));
    }
}

#[test]
fn view_change_redisplays_unchanged_visible_placement() {
    let mut cache = HostGraphicsCache::default();
    update(&mut cache, &[test_placement(0, 0)], false);
    assert_eq!(cache.placements.len(), 1);
    let bytes = update(&mut cache, &[test_placement(0, 0)], true);
    assert!(!String::from_utf8_lossy(&bytes).contains("a=t"));
    assert!(String::from_utf8_lossy(&bytes).contains("a=p"));
    assert_eq!(cache.placements.len(), 1);
}

#[test]
fn surface_reset_deletes_then_reuploads_and_redisplays_placement() {
    let mut cache = HostGraphicsCache::default();
    update(&mut cache, &[test_placement(0, 0)], false);
    assert_eq!((cache.images.len(), cache.placements.len()), (1, 1));
    let mut bytes = cache.clear_bytes();
    bytes.extend(update(&mut cache, &[test_placement(0, 0)], false));
    let redisplay = String::from_utf8_lossy(&bytes);
    assert!(redisplay.contains("a=d,d=I"));
    assert!(redisplay.contains("a=t"));
    assert!(redisplay.contains("a=p"));
    assert_eq!((cache.images.len(), cache.placements.len()), (1, 1));
}

#[test]
fn scrollback_offset_change_redisplays_placement() {
    let mut cache = HostGraphicsCache::default();
    update(&mut cache, &[test_placement(0, 0)], false);
    let mut scrolled = test_placement(0, 0);
    scrolled.scrollback_offset = 3;
    let bytes = update(&mut cache, &[scrolled], false);
    assert!(!String::from_utf8_lossy(&bytes).contains("a=t"));
    assert!(String::from_utf8_lossy(&bytes).contains("a=p"));
}

#[test]
fn changing_first_terminal_source_does_not_starve_second_source() {
    let terminal = |id| {
        let mut placement = test_placement(0, 0);
        placement.placement.image_id = id;
        placement.placement.data_fingerprint = u64::from(id);
        placement.source_key = HostSourceKey::Terminal {
            pane_id: placement.pane_id,
            image_id: id,
        };
        placement
    };
    let second = terminal(99).source_key;
    let mut cache = HostGraphicsCache::default();
    for id in 1..=3 {
        assert!(
            encode_graphics_update_incremental(
                &mut cache,
                &[terminal(id), terminal(99)],
                None,
                false,
            )
            .incomplete
        );
    }
    assert!(cache.sources.contains_key(&second));
}

#[test]
fn large_terminal_image_is_local_but_quarantined_headless() {
    let placements = || {
        let mut large = test_placement(0, 0);
        large.placement.data_len = 24 * 1024 * 1024;
        let mut later = test_placement(4, 0);
        later.placement.image_id = 8;
        later.source_key = HostSourceKey::Terminal {
            pane_id: later.pane_id,
            image_id: 8,
        };
        [large, later]
    };
    for (budget, expected) in [
        (None, (true, 1, 0)),
        (Some(HEADLESS_GRAPHICS_TRANSACTION_BUDGET), (false, 1, 1)),
    ] {
        let mut cache = HostGraphicsCache::default();
        let encoded = encode_graphics_update_incremental(&mut cache, &placements(), budget, false);
        assert!(String::from_utf8_lossy(&encoded.bytes).contains("a=t"));
        assert_eq!(
            (
                encoded.incomplete,
                cache.images.len(),
                cache.oversized.len()
            ),
            expected
        );
    }
}

#[test]
fn terminal_image_data_requests_deduplicate_and_reconsider_changed_signatures() {
    let pane_id = PaneId::from_raw(1);
    let descriptor = KittyImageDescriptor {
        image_id: 7,
        placement_id: 1,
        image_width: 3456,
        image_height: 2234,
        format: KittyImageFormat::Rgba,
        data_len: 3456 * 2234 * 4,
        data_fingerprint: 42,
    };
    let mut requested = HashSet::new();
    assert!(terminal_image_needs_data(
        pane_id,
        descriptor,
        &HashMap::new(),
        &HashMap::new(),
        &mut requested,
    ));
    let mut second_placement = descriptor;
    second_placement.placement_id = 2;
    assert!(!terminal_image_needs_data(
        pane_id,
        second_placement,
        &HashMap::new(),
        &HashMap::new(),
        &mut requested,
    ));

    let signature = image_signature_from_descriptor(descriptor, 32);
    let source = HostSourceKey::Terminal {
        pane_id,
        image_id: descriptor.image_id,
    };
    let oversized = HashMap::from([(source, signature)]);
    let mut requested = HashSet::new();
    assert!(!terminal_image_needs_data(
        pane_id,
        descriptor,
        &HashMap::new(),
        &oversized,
        &mut requested,
    ));
    let mut changed = descriptor;
    changed.data_fingerprint += 1;
    assert!(terminal_image_needs_data(
        pane_id,
        changed,
        &HashMap::new(),
        &oversized,
        &mut requested,
    ));
}
