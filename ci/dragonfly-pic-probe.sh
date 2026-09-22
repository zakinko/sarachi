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

ROOT=$(pwd)
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

# rust の build が使っているのと同じ cc に固定する。
#
# 前の回は単独で建てて「正常」と出たが、使われていたのは cc 1.4.7 だった。
# rust 1.86 が cargo を建てるのに使うのは 1.1.22 で、object の名前の付け方
# からして別物（1.1.22 は agent.o、1.4.7 は <hash>-agent.o）。版を揃えない
# 比較には意味が無い。
CCVER=${SARACHI_CC_VER:-1.1.22}
cargo generate-lockfile > /dev/null 2>&1 || true
if cargo update -p cc --precise "$CCVER" > "$W/pin.log" 2>&1; then
	echo "  cc を $CCVER に固定した"
else
	echo "  cc を固定できなかった:"
	tail -3 "$W/pin.log" | sed 's/^/    /'
fi
grep -A1 '^name = "cc"' Cargo.lock | sed 's/^/    /'

# bootstrap が立てるのと同じ環境変数を再現する。
#
# rust の bootstrap は CFLAGS_<triple を下線にした物> を立てる
# （src/bootstrap/src/core/builder/cargo.rs）。dragonfly 向けに足す物は
# 空なので、値は空文字列になる。**環境変数が「立っている」ことそのものが
# cc-rs の既定を変えるのではないか**を見る。
if [ -n "${SARACHI_SET_CFLAGS:-}" ]; then
	CFLAGS_x86_64_unknown_dragonfly="$SARACHI_EMPTY_OK"
	export CFLAGS_x86_64_unknown_dragonfly
	echo "  CFLAGS_x86_64_unknown_dragonfly を立てた（値: '${SARACHI_EMPTY_OK}'）"
fi

if cargo build -vv > "$W/build.log" 2>&1; then
	echo "  [通った] cargo 単独では建つ（bin まで）"
	BUILT=yes
else
	echo "  [駄目] cargo 単独でも落ちる"
	BUILT=no
fi

say "libssh2-sys の OUT_DIR に何が在るか"
OUT=$(find "$W" -type d -name 'libssh2-sys-*' 2>/dev/null | head -3)
if [ -n "$OUT" ]; then
	for d in $OUT; do
		echo "  $d"
		find "$d" -name '*.o' 2>/dev/null | head -5 | sed 's/^/    /'
		find "$d" -name '*.a' 2>/dev/null | head -3 | sed 's/^/    /'
		n=$(find "$d" -name '*.o' 2>/dev/null | wc -l)
		echo "    object の数: $n"
	done
else
	echo "  libssh2-sys の build ディレクトリが無い"
fi

say "agent.c を建てた command line"
grep -oE '"cc"[^\n]*agent\.c[^\n]*' "$W/build.log" | head -1 | cut -c1-500 | sed 's/^/  /' \
	|| echo "  agent.c を建てた形跡が無い"

say "その command line に -fPIC が在るか"
if grep -oE '"cc"[^\n]*agent\.c[^\n]*' "$W/build.log" | head -1 | grep -q -- '-fPIC'; then
	echo "  **在る**"
else
	echo "  無い（agent.c を建てていないだけかもしれない）"
fi

say "出来た object の relocation"
O=$(find "$W" -name '*agent*.o' 2>/dev/null | head -1)
if [ -n "$O" ]; then
	echo "  $O"
	if command -v readelf > /dev/null 2>&1; then
		readelf -r "$O" 2>/dev/null | awk '$3 ~ /R_X86_64_32$/' | wc -l \
			| awk '{print "    R_X86_64_32 (PIC でない): "$1" 件"}'
		readelf -r "$O" 2>/dev/null | awk '$3 ~ /GOTPCREL|R_X86_64_PC32/' | wc -l \
			| awk '{print "    PIC 向け: "$1" 件"}'
	fi
else
	echo "  無い。どこから libssh2 を得たかを見る"
	grep -iE 'rustc-link-lib|rustc-link-search|pkg_config|LIBSSH2' "$W/build.log" \
		| grep -i ssh | head -5 | sed 's/^/    /'
fi

say "落ちた場合の理由"
grep -E 'relocation|recompile|error\[|error:' "$W/build.log" | head -8 | sed 's/^/  /' || true

say "build.log を持ち帰る"
# 部分的に grep して当たらなければ何も見えない、という形で二度外した。
# log そのものを持ち帰って、手元で読む。
mkdir -p "$ROOT/probe-out"
cp "$W/build.log" "$ROOT/probe-out/build.log"
wc -l < "$W/build.log" | awk '{print "  "$1" 行を持ち帰る"}'
# object も一つ持ち帰る。relocation を手元で数え直せる。
O=$(find "$W" -name '*agent*.o' 2>/dev/null | head -1)
[ -n "$O" ] && cp "$O" "$ROOT/probe-out/agent.o" && echo "  agent.o も持ち帰る"

say "片付け"
cd "$HOME"; rm -rf "$W"
echo "  済"
