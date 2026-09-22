#!/bin/sh
# DragonFly で -sys crate の C に -fPIC が付かない件を、測って突き止める。
#
# ここまでで分かっていること（どれも「付くはず」を指している）。
#   cc 1.1.22 は dragonfly に -fPIC を付ける（除外は windows / -none- / uefi）
#   rust の bootstrap は CRATE_CC_NO_DEFAULTS を立てていない
#   -fPIC は cc-rs 自身が付ける側にあり、bootstrap が渡す CFLAGS_<triple> に
#   は含まれない
#   libssh2-sys 0.3.1 の build.rs に .pic(false) は無い
#
# それなのに付いていない。理屈で詰められないので、実際に叩かれた command line
# と出来た object を見る。
#
# 知りたいのは二つ。
#   1. cargo 単独でも再現するか（するなら cc-rs か libssh2-sys、しないなら
#      rust の bootstrap の側）
#   2. cc-rs が実際に何を渡しているか
set -e
say() { echo; echo "### $*"; }

W=$HOME/pic-probe
rm -rf "$W"; mkdir -p "$W"; cd "$W"

say "道具"
rustc --version
cargo --version
cc --version | head -1

say "libssh2-sys を単独で建てる"
#
# 一度目の下調べは二つ足りていなかった。
#
#   cargo は build script の出力を成功時に表示しない（-vv が要る）
#   lib を建てるだけでは、あの relocation error は表に出ない。PIE の実行体を
#   link するときに出るものなので、bin を建てる必要がある
#
# 「再現しなかった」のではなく、再現する条件を作っていなかった。
mkdir p; cd p
cat > Cargo.toml <<'CONF'
[package]
name = "picprobe"
version = "0.0.0"
edition = "2021"

[dependencies]
libssh2-sys = "0.3.1"

[[bin]]
name = "picprobe"
path = "src/main.rs"

[workspace]
CONF
mkdir src
# 実際に使う。使わないと link から落とされて、確かめたい所を通らない。
cat > src/main.rs <<'RS'
fn main() {
    unsafe {
        libssh2_sys::libssh2_init(0);
    }
    println!("ok");
}
RS

CC_ENABLE_DEBUG_OUTPUT=1
export CC_ENABLE_DEBUG_OUTPUT
if cargo build -vv > "$W/build.log" 2>&1; then
	echo "  [通った] cargo 単独では建つ（bin まで）"
	BUILT=yes
else
	echo "  [駄目] cargo 単独でも落ちる"
	BUILT=no
fi

say "cc-rs が叩いた command line"
grep -m2 -oE '"cc"[^\n]*' "$W/build.log" | cut -c1-400 | sed 's/^/  /' \
	|| grep -m2 'running:' "$W/build.log" | cut -c1-400 | sed 's/^/  /' \
	|| echo "  出ていない"

say "-fPIC が渡っているか"
if grep -q -- '-fPIC' "$W/build.log"; then
	grep -o -- '-fPIC' "$W/build.log" | wc -l | awk '{print "  渡っている（"$1" 回）"}'
else
	echo "  **渡っていない**"
fi

say "出来た object の relocation"
O=$(find "$W" -name 'agent.o' 2>/dev/null | head -1)
if [ -n "$O" ]; then
	echo "  $O"
	if command -v readelf > /dev/null 2>&1; then
		readelf -r "$O" 2>/dev/null | awk '$3 ~ /R_X86_64_32$/' | wc -l \
			| awk '{print "    R_X86_64_32 (PIC でない): "$1" 件"}'
		readelf -r "$O" 2>/dev/null | awk '$3 ~ /GOTPCREL|R_X86_64_PC32/' | wc -l \
			| awk '{print "    PIC 向け: "$1" 件"}'
	else
		echo "    readelf が無い"
	fi
else
	echo "  agent.o が無い。C を建てていない——どこから libssh2 を得たかを見る"
	grep -iE 'pkg-config|pkgconfig|rustc-link-lib|rustc-link-search' "$W/build.log" \
		| head -5 | sed 's/^/    /'
fi

say "落ちた場合の理由"
grep -E 'relocation|recompile|error\[|error:' "$W/build.log" | head -8 | sed 's/^/  /' || true

say "片付け"
cd "$HOME"; rm -rf "$W"
echo "  済"
