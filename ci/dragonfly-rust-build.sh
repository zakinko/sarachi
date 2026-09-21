#!/bin/sh
# DragonFly 向けの rustc / rust-std / cargo を建てて dist/ に置く。
#
#   ci/dragonfly-rust-build.sh <version> <llvm package> [bootstrap url]
#
# bootstrap を省くと pkg の rust (1.85.1) を種にする。二段目からは前の段が
# 出した tarball の置き場を渡す。rustc は一つ前の版でしか建たないので、
# 1.85.1 → 1.86 → 1.87 → 1.88 と順に積むことになる。
set -e

VERSION=$1
LLVMPKG=$2
BOOTSTRAP_URL=$3
BOOTSTRAP_VER=$4

[ -n "$VERSION" ] || { echo "版が要る" >&2; exit 1; }
[ -n "$LLVMPKG" ] || { echo "LLVM の package 名が要る" >&2; exit 1; }
if [ -n "$BOOTSTRAP_URL" ] && [ -z "$BOOTSTRAP_VER" ]; then
	echo "置き場を渡すなら、種の版も要る" >&2
	exit 1
fi

TRIPLE=x86_64-unknown-dragonfly
OUT=$(pwd)/dist
# / は 134G あるが、/build は build のために切られた別の partition なので
# そちらを使う。45G あり、rustc には十分。
WRK=/build/rust
rm -rf "$WRK"; mkdir -p "$WRK" "$OUT"

say() { echo; echo "### $* ($(date '+%H:%M:%S'))"; }

say "llvm-config を探す"
# DPorts の llvm は版つきの名前で入る (llvm-config20 など)。名前を決め打ちに
# すると package の付け方が変わったときに黙って同梱 LLVM を建て始めるので、
# 見つからなければここで止める。
SUFFIX=$(echo "$LLVMPKG" | tr -dc '0-9')
LLVM_CONFIG=""
for c in "/usr/local/bin/llvm-config${SUFFIX}" "/usr/local/llvm${SUFFIX}/bin/llvm-config"; do
	[ -x "$c" ] && { LLVM_CONFIG=$c; break; }
done
[ -n "$LLVM_CONFIG" ] || { echo "llvm-config が見つからない ($LLVMPKG)" >&2; ls /usr/local/bin | grep -i llvm >&2; exit 1; }
echo "  $LLVM_CONFIG ($("$LLVM_CONFIG" --version))"

say "種にする rustc を決める"
if [ -n "$BOOTSTRAP_URL" ]; then
	# 前の段が出した tarball を入れて、それを種にする。置き場には
	# rustc- rust-std- cargo- の三つが <part>-<ver>-<triple>.tar.xz の名前で
	# 並んでいる前提。mneumann 氏が leaf.dragonflybsd.org で配っている形に
	# 合わせてあるので、将来そちらへ戻す時にも同じ名前で済む。
	BOOT=$WRK/bootstrap
	mkdir -p "$BOOT" "$WRK/boottar"
	cd "$WRK/boottar"
	echo "  ${BOOTSTRAP_URL%/} から ${BOOTSTRAP_VER} を取る"
	for part in rustc rust-std cargo; do
		f="${part}-${BOOTSTRAP_VER}-${TRIPLE}.tar.xz"
		curl -sfL -O "${BOOTSTRAP_URL%/}/${f}"
		tar xf "$f"
		(cd "${f%.tar.xz}" && ./install.sh --prefix="$BOOT" --disable-ldconfig)
	done
	cd "$WRK"
else
	BOOT=/usr/local
	echo "  pkg の rust を使う: $(rustc --version)"
fi
RUSTC_BIN=$BOOT/bin/rustc
CARGO_BIN=$BOOT/bin/cargo
[ -x "$RUSTC_BIN" ] || { echo "種の rustc が無い: $RUSTC_BIN" >&2; exit 1; }
echo "  種: $("$RUSTC_BIN" --version)"

say "rustc ${VERSION} の source を取る"
cd "$WRK"
SRC=rustc-${VERSION}-src.tar.xz
curl -sfL -O "https://static.rust-lang.org/dist/${SRC}"
curl -sfL -O "https://static.rust-lang.org/dist/${SRC}.sha256"
# 配布物の取り違えは build の何時間も先で妙な形で出るので、ここで止める。
# DragonFly の sha256 は FreeBSD と違って -c を持たない (getopt は
# "hb:e:pqrs:tx")。-q で値を出して自分で比べる。
WANT=$(awk '{print $1}' "${SRC}.sha256")
GOT=$(sha256 -q "$SRC")
if [ "$WANT" != "$GOT" ]; then
	echo "sha256 が合わない: 期待 $WANT 実際 $GOT" >&2
	exit 1
fi
echo "  sha256 一致"
tar xf "$SRC"
cd "rustc-${VERSION}-src"

say "config.toml を書く"
cat > config.toml <<CONF
[llvm]
# 同梱の LLVM を建てると 4 core では二時間以上かかり、job の上限に収まらない。
# package の LLVM に向ける。
download-ci-llvm = false
link-shared = true

[build]
build = "${TRIPLE}"
host = ["${TRIPLE}"]
target = ["${TRIPLE}"]
rustc = "${RUSTC_BIN}"
cargo = "${CARGO_BIN}"
python = "python3"
docs = false
extended = true
tools = ["cargo"]
vendor = true

[install]
prefix = "/usr/local"

[rust]
channel = "stable"
# 配る物に debug 情報は要らない。build 時間と成果物の大きさの両方に効く。
debug = false
debug-assertions = false
codegen-units = 1

[target.${TRIPLE}]
llvm-config = "${LLVM_CONFIG}"
CONF
cat config.toml | sed 's/^/  /'

say "建てる"
# ld.gold では std を rustc driver に link できない。DPorts の
# Makefile.DragonFly が同じ理由で ld.bfd を指定している。
LDVER=ld.bfd
export LDVER
LD_LIBRARY_PATH=$BOOT/lib:/usr/lib/gcc80
export LD_LIBRARY_PATH
python3 x.py dist rustc rust-std cargo

say "成果物を集める"
find build -name "*-${TRIPLE}.tar.xz" -print | sed 's/^/  /'
for part in rustc rust-std cargo; do
	f=$(find build -name "${part}-*-${TRIPLE}.tar.xz" | head -1)
	[ -n "$f" ] || { echo "  !!! ${part} の tarball が無い" >&2; exit 1; }
	cp "$f" "$OUT/"
done
ls -l "$OUT" | sed 's/^/  /'
