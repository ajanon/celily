from builder import Distro


class Arch(Distro):
    name = "archlinux"
    oci_image = "docker.io/library/archlinux:base-devel"

    def setup(self):
        return [["pacman", "-Syu", "--noconfirm", "--needed", "rust"]]
