from builder import Distro


class Ubuntu(Distro):
    """The archive's default Rust is too old (1.75 in 24.04): `rust` is a
    versioned toolchain from -updates, rustc-X.Y and cargo-X.Y. Its point
    release must reach the highest rust-version in Cargo.lock."""

    def __init__(self, version, *, rust):
        self.version = version
        self.rust = rust

    @property
    def name(self):
        return f"ubuntu-{self.version}"

    @property
    def oci_image(self):
        return f"docker.io/library/ubuntu:{self.version}"

    def setup(self):
        return [
            ["apt-get", "update"],
            [
                "apt-get",
                "install",
                "--yes",
                "--no-install-recommends",
                "build-essential",
                # cargo fetches crates over TLS, with the system's CAs.
                "ca-certificates",
                f"rustc-{self.rust}",
                f"cargo-{self.rust}",
            ],
        ]

    def env(self):
        return {"DEBIAN_FRONTEND": "noninteractive", "RUSTC": f"rustc-{self.rust}"}

    def cargo(self):
        return f"cargo-{self.rust}"
