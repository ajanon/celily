from image_build import Guest


class Arch(Guest):
    distro = "arch"
    source = "images:archlinux"

    def fix_boot(self, instance):
        # No console in an LXD container: it fails, the system is degraded.
        instance.run(["systemctl", "mask", "console-getty.service"])

    def wait_for_network(self, instance):
        # networkd configures eth0; the boot does not wait for it.
        instance.run(["/usr/lib/systemd/systemd-networkd-wait-online", "--timeout=60"])

    def install_ca_tools(self, instance):
        instance.run(
            [
                "pacman",
                "-Syu",
                "--noconfirm",
                "--needed",
                "ca-certificates",
                "ca-certificates-utils",
            ]
        )
