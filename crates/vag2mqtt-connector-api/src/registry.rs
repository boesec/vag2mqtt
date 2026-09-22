//! The compiled-in brands.

use std::collections::HashMap;
use std::sync::Arc;

use vag2mqtt_domain::Brand;

use crate::connector::Connector;

/// The connectors this build ships, one per brand. Filled by the binary, read by the runtime.
#[derive(Clone, Default)]
pub struct ConnectorRegistry {
    by_brand: HashMap<Brand, Arc<dyn Connector>>,
}

/// A brand was registered twice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("connector for brand {0} registered twice")]
pub struct DuplicateBrand(pub Brand);

impl ConnectorRegistry {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a connector under the brand it reports in [`Connector::info`].
    pub fn register(mut self, connector: Arc<dyn Connector>) -> Result<Self, DuplicateBrand> {
        let brand = connector.info().brand;
        if self.by_brand.contains_key(&brand) {
            return Err(DuplicateBrand(brand));
        }
        self.by_brand.insert(brand, connector);
        Ok(self)
    }

    /// The connector for a brand, if this build has one.
    pub fn get(&self, brand: Brand) -> Option<Arc<dyn Connector>> {
        self.by_brand.get(&brand).cloned()
    }

    /// The brands with a connector, sorted, for the UI's dropdown.
    pub fn brands(&self) -> Vec<Brand> {
        let mut brands: Vec<Brand> = self.by_brand.keys().copied().collect();
        brands.sort();
        brands
    }

    /// `true` if no connector is registered.
    pub fn is_empty(&self) -> bool {
        self.by_brand.is_empty()
    }
}

impl std::fmt::Debug for ConnectorRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConnectorRegistry")
            .field("brands", &self.brands())
            .finish()
    }
}
