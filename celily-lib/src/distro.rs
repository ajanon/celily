/// Which Linux distribution the image is based on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "lowercase"))]
pub enum DistroKind {
    /// Arch Linux.
    #[default]
    Arch,
}

impl DistroKind {
    /// Directory inside the container where additional CA certificates are
    /// placed (absolute path, no trailing slash).
    pub fn ca_cert_anchors_dir(self) -> &'static str {
        match self {
            DistroKind::Arch => "/etc/ca-certificates/trust-source/anchors",
        }
    }

    /// Command to rebuild the system CA trust store after a new certificate
    /// has been placed in the anchors directory.
    pub fn rebuild_trust_store_command(self) -> &'static str {
        match self {
            DistroKind::Arch => "update-ca-trust",
        }
    }
}
