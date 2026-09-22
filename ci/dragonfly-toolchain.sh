#!/bin/sh
# DragonFly に、自前で建てた rustc を入れる。
#
# DPorts が配るのは 1.85.1 で 2025-04-05 から動いていない。sarachi 本体は
# is_multiple_of (1.87 以降) を使うので、それでは建たない。libhimmelblau は
# 1.88 を要る。どちらも配布 rust では足りないので、建てた物を置き場から取る。
#
# 建て方は .github/workflows/dragonfly-rust.yml にある。版を上げるときは
# そちらで新しい段を建て、release に足してから、ここの VER を変える。
set -e

VER=1.88.0
TRIPLE=x86_64-unknown-dragonfly
BASE=https://github.com/zakinko/netbsd-ci-images/releases/download/dragonfly-rust

W=$(pwd)/.ci-toolchain
rm -rf "$W"; mkdir -p "$W"; cd "$W"

for part in rustc rust-std cargo; do
	f="${part}-${VER}-${TRIPLE}.tar.xz"
	echo "### $f"
	curl -sfL -O "${BASE}/${f}"
	tar xf "$f"
	# install.sh は #!/usr/bin/env bash で始まる。DragonFly の base に bash は
	# 無いので、呼ぶ側で入れてある。
	(cd "${f%.tar.xz}" && ./install.sh --prefix=/usr/local --disable-ldconfig)
done

cd ..
rm -rf "$W"
rustc --version
cargo --version
