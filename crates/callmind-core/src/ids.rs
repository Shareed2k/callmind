use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;
use uuid::Uuid;

macro_rules! define_uuid_id {
    ($name:ident, $doc:expr) => {
        #[doc = $doc]
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, utoipa::ToSchema)]
        #[serde(transparent)]
        #[schema(value_type = String, format = "uuid", example = "a1b2c3d4-e5f6-7a8b-9c0d-1e2f3a4b5c6d")]
        pub struct $name(pub Uuid);

        impl $name {
            /// Generate a new random UUID v4 identifier.
            pub fn generate() -> Self {
                Self(Uuid::new_v4())
            }

            /// Return the underlying UUID reference.
            pub fn as_uuid(&self) -> &Uuid {
                &self.0
            }

            /// Return the underlying UUID.
            pub fn into_inner(self) -> Uuid {
                self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::generate()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl From<Uuid> for $name {
            fn from(uuid: Uuid) -> Self {
                Self(uuid)
            }
        }

        impl From<$name> for Uuid {
            fn from(id: $name) -> Self {
                id.0
            }
        }

        impl FromStr for $name {
            type Err = uuid::Error;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Uuid::parse_str(s).map(Self)
            }
        }
    };
}

define_uuid_id!(CallId, "Strongly typed unique identifier for a Call.");
define_uuid_id!(
    RecordingId,
    "Strongly typed unique identifier for an Audio Recording."
);
define_uuid_id!(
    JobId,
    "Strongly typed unique identifier for a Background Job."
);
define_uuid_id!(
    OrgId,
    "Strongly typed unique identifier for an Organization / Tenant."
);

/// Strongly typed speaker identifier within a conversation (0, 1, 2, ...).
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
    utoipa::ToSchema,
)]
#[serde(transparent)]
pub struct SpeakerId(pub u16);

impl SpeakerId {
    pub fn new(id: u16) -> Self {
        Self(id)
    }

    pub fn inner(self) -> u16 {
        self.0
    }

    pub fn as_u16(self) -> u16 {
        self.0
    }
}

impl fmt::Display for SpeakerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "speaker_{}", self.0)
    }
}

impl From<u16> for SpeakerId {
    fn from(val: u16) -> Self {
        Self(val)
    }
}

impl From<SpeakerId> for u16 {
    fn from(s: SpeakerId) -> Self {
        s.0
    }
}

impl OrgId {
    /// The single organization seeded by the initial migration.
    ///
    /// Was a `&str` const duplicated across five call sites, each re-parsed at
    /// runtime with `Uuid::parse_str(..).unwrap()`. `uuid!` parses at compile
    /// time, so the unwraps are gone.
    pub const DEFAULT: Self = Self(uuid::uuid!("00000000-0000-0000-0000-000000000001"));
}
