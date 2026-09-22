//! Service and inspection intervals.

use serde::{Deserialize, Serialize};

use crate::reading::Reading;
use crate::units::{Days, ServiceKilometres};

/// When the next service is due, as the manufacturer reports it: a countdown in days and in
/// kilometres. Negative values mean overdue. The domain does not turn the days into a date,
/// because that would invent a precision the source does not have.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Service {
    /// Days until the next inspection.
    pub inspection_due_in: Reading<Days>,
    /// Kilometres until the next inspection.
    pub inspection_due_after: Reading<ServiceKilometres>,
    /// Days until the next oil service.
    pub oil_service_due_in: Reading<Days>,
    /// Kilometres until the next oil service.
    pub oil_service_due_after: Reading<ServiceKilometres>,
}

impl Service {
    /// Every value unsupported.
    pub fn unsupported() -> Self {
        Self {
            inspection_due_in: Reading::Unsupported,
            inspection_due_after: Reading::Unsupported,
            oil_service_due_in: Reading::Unsupported,
            oil_service_due_after: Reading::Unsupported,
        }
    }
}
