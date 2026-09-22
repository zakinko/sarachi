#!/bin/sh
# 建てた rustc で libhimmelblau が実際に建つことを、DragonFly 実機で示す。
#
#   ci/dragonfly-libhimmelblau.sh <rustc の版> <tarball の置き場>
#
# 三段積んで 1.88 を作ったのは libhimmelblau のためなので、それが建たなければ
# 意味が無い。rustc が出来たことと、目的が達せられたことは別。
#
# cross では確かめられない。openssl-sys aws-lc-sys ring と C のライブラリを
# 要り、DragonFly の base は LibreSSL 3.6.1 なので実機で建てるしかない。
set -e

VER=$1
BOOTDIR=$2
KR_VER=0.1.0
HB_VER=0.8.41
TRIPLE=x86_64-unknown-dragonfly

[ -n "$VER" ] || { echo "版が要る" >&2; exit 1; }
[ -d "$BOOTDIR" ] || { echo "置き場が無い: $BOOTDIR" >&2; exit 1; }
BOOTDIR=$(cd "$BOOTDIR" && pwd)

ROOT=$(pwd)
PATCHFILE=$ROOT/patches/libkrimes-${KR_VER}-bsd-getdomainname.diff
[ -f "$PATCHFILE" ] || { echo "当て物が無い: $PATCHFILE" >&2; exit 1; }

W=/build/hb
rm -rf "$W"; mkdir -p "$W"; cd "$W"

say() { echo; echo "### $* ($(date '+%H:%M:%S'))"; }

say "建てた rustc ${VER} を入れる"
TC=$W/toolchain
mkdir -p "$TC"
for part in rustc rust-std cargo; do
	f="${part}-${VER}-${TRIPLE}.tar.xz"
	cp "$BOOTDIR/$f" .
	tar xf "$f"
	(cd "${f%.tar.xz}" && ./install.sh --prefix="$TC" --disable-ldconfig)
done
PATH=$TC/bin:$PATH
export PATH
rustc --version
cargo --version

say "libkrimes ${KR_VER} を取って当てる"
curl -sfL -O "https://static.crates.io/crates/libkrimes/libkrimes-${KR_VER}.crate"
tar xf "libkrimes-${KR_VER}.crate"
(cd "libkrimes-${KR_VER}" && patch -f -p0 -i "$PATCHFILE" < /dev/null)

say "libhimmelblau ${HB_VER} を要る package を作る"
mkdir probe; cd probe
cat > Cargo.toml <<CONF
[package]
name = "hb-probe"
version = "0.0.0"
edition = "2021"

[dependencies]
libhimmelblau = "${HB_VER}"

[patch.crates-io]
libkrimes = { path = "$W/libkrimes-${KR_VER}" }

[workspace]
CONF
mkdir src; echo 'pub fn probe() {}' > src/lib.rs

say "建てる"
# DragonFly は既定で PIE を作るので、C を建てる -sys crate に -fPIC を渡す。
# bootstrap を通らない素の cargo なので、素の CFLAGS で届く。
CFLAGS="-fPIC"
CXXFLAGS="-fPIC"
export CFLAGS CXXFLAGS
cargo build

RLIB=$(find target -name 'libhimmelblau*.rlib' | head -1)
[ -n "$RLIB" ] || { echo "  !!! 成果物が無い" >&2; exit 1; }
echo "  建った: $(wc -c < "$RLIB") bytes"

say "何を link したか"
grep -E '^name = "(openssl-sys|aws-lc-sys|ring|rustls)"' Cargo.lock || true
