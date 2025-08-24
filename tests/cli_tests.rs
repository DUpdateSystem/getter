use getter::websdk::repo::provider::get_hub_uuid;

#[test]
fn test_provider_accepts_friendly_names() {
    // This test verifies that the provider system now accepts friendly names directly
    // through the merged PROVIDER_MAP structure
    assert_eq!(
        get_hub_uuid("github"),
        "fd9b2602-62c5-4d55-bd1e-0d6537714ca0"
    );
    assert_eq!(
        get_hub_uuid("fdroid"),
        "6a6d590b-1809-41bf-8ce3-7e3f6c8da945"
    );
}

#[test]
fn test_cli_hub_mapping() {
    // Test that CLI can correctly map user-friendly names to UUIDs
    let test_cases = vec![
        ("github", "fd9b2602-62c5-4d55-bd1e-0d6537714ca0"),
        ("fdroid", "6a6d590b-1809-41bf-8ce3-7e3f6c8da945"),
        ("gitlab", "a84e2fbe-1478-4db5-80ae-75d00454c7eb"),
        ("lsposed", "401e6259-2eab-46f0-8e8a-d2bfafedf5bf"),
    ];

    for (friendly_name, expected_uuid) in test_cases {
        let actual_uuid = get_hub_uuid(friendly_name);
        assert_eq!(
            actual_uuid, expected_uuid,
            "Failed to map {} to {}, got {}",
            friendly_name, expected_uuid, actual_uuid
        );
    }
}

#[test]
fn test_cli_passthrough_uuid() {
    // Test that existing UUIDs pass through unchanged
    let uuid = "fd9b2602-62c5-4d55-bd1e-0d6537714ca0";
    assert_eq!(get_hub_uuid(uuid), uuid);

    // Test that random strings pass through unchanged
    let random = "some-random-string";
    assert_eq!(get_hub_uuid(random), random);
}

#[test]
fn test_all_supported_hubs() {
    // Test that all expected hubs are supported
    let supported_hubs = vec!["github", "fdroid", "gitlab", "lsposed"];

    for hub in supported_hubs {
        let uuid = get_hub_uuid(hub);
        // UUID should not be the same as the input (meaning it was mapped)
        assert_ne!(uuid, hub, "Hub {} was not mapped to a UUID", hub);

        // UUID should be a valid UUID format (basic check)
        assert!(
            uuid.len() == 36,
            "UUID {} for hub {} is not 36 characters",
            uuid,
            hub
        );
        assert!(
            uuid.chars().filter(|&c| c == '-').count() == 4,
            "UUID {} for hub {} doesn't have 4 hyphens",
            uuid,
            hub
        );
    }
}
