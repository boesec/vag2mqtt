//! Registry behaviour, checked from outside the crate with the fake connector.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use vag2mqtt_connector_api::fake::FakeConnector;
use vag2mqtt_connector_api::{ConnectorRegistry, DuplicateBrand};
use vag2mqtt_domain::Brand;

#[test]
fn registry_lists_brands_and_rejects_duplicates() {
    let (audi, _) = FakeConnector::builder(Brand::Audi).build();
    let (skoda, _) = FakeConnector::builder(Brand::Skoda).build();
    let registry = ConnectorRegistry::new()
        .register(Arc::new(skoda))
        .unwrap()
        .register(Arc::new(audi.clone()))
        .unwrap();
    assert_eq!(registry.brands(), vec![Brand::Audi, Brand::Skoda]);
    assert!(registry.get(Brand::Audi).is_some());
    assert!(registry.get(Brand::Cupra).is_none());
    assert!(!registry.is_empty());
    assert_eq!(
        format!("{registry:?}"),
        "ConnectorRegistry { brands: [Audi, Skoda] }"
    );

    let duplicate = registry.register(Arc::new(audi)).unwrap_err();
    assert_eq!(duplicate, DuplicateBrand(Brand::Audi));
}

#[test]
fn empty_registry() {
    let registry = ConnectorRegistry::new();
    assert!(registry.is_empty());
    assert!(registry.brands().is_empty());
}
