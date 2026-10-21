#!/bin/bash
# build-desktop-kernel wrapper（规避 git-bash → wsl 的 $PATH 展开括号坑）
cd /root/vmbuild || exit 1
export PATH="$HOME/.cargo/bin:$PATH"
exec bash plugins/plugin-vm/build-vm-image.sh --variant desktop --skip-download --out target/vm-image --apt-mirror https://mirrors.ustc.edu.cn/debian --apt-suite bookworm --guest-bin bin/bin_guest/x86_64/nemesis-guest --base-url https://cloud.debian.org/images/cloud/bookworm/latest/debian-12-genericcloud-amd64.qcow2 --sha256 c0a478483fb1a6c098b79edd27e6ec25db63dd359896e87dcf5c91e07b5fd503
