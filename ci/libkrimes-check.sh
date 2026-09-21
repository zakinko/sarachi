#!/bin/sh
# 当て物が要ることと、当てれば建つことを、その platform で実際に示す。
#
# 当て物が名指しする platform は一つずつ動かす、という決まりがある。cross では
# 確かめられない物があるので（openssl-sys は標的側の OpenSSL を要る）、実機で
# これを走らせる。
#
# patch には -i で当て物を渡し、stdin は /dev/null に向ける。両方を < で繋ぐと
# 後の方が勝って当て物が渡らない。-f と合わせて、当たらなかったときに
# "File to patch:" と問い返して止まらなくなるのも防ぐ。
set -e

VER=0.1.0
ROOT=$(pwd)
PATCHFILE=$ROOT/patches/libkrimes-${VER}-bsd-getdomainname.diff
W=$ROOT/.ci-libkrimes

[ -f "$PATCHFILE" ] || { echo "当て物が無い: $PATCHFILE" >&2; exit 1; }
rm -rf "$W"; mkdir -p "$W"; cd "$W"

echo "### libkrimes ${VER} を取る"
URL="https://static.crates.io/crates/libkrimes/libkrimes-${VER}.crate"
if command -v fetch >/dev/null 2>&1; then fetch -o libkrimes.crate "$URL"
elif command -v ftp >/dev/null 2>&1; then ftp -o libkrimes.crate "$URL"
else curl -sfL -o libkrimes.crate "$URL"
fi
tar xzf libkrimes.crate
cd "libkrimes-${VER}"

# 展開先がこの repo の下なので、そのままだと cargo が workspace の一員だと
# 思って断る。空の [workspace] を足して単独の package にする。
printf '\n[workspace]\n' >> Cargo.toml

echo
echo "### 当てる前は E0425 で落ちること"
set +e
cargo build > ../before.log 2>&1
BEFORE=$?
set -e
if [ "$BEFORE" -eq 0 ]; then
	echo "  !!! 当て物なしで建った。この platform には要らないかもしれない"
	exit 1
fi
if grep -q 'E0425' ../before.log; then
	echo "  期待どおり E0425 で落ちた"
else
	echo "  !!! E0425 以外の理由で落ちた"
	grep -E '^error' ../before.log | head -5
	exit 1
fi

echo
echo "### 当てる"
patch -f -p0 -i "$PATCHFILE" < /dev/null

echo
echo "### 当てた後は建つこと"
set +e
cargo build > ../after.log 2>&1
AFTER=$?
set -e
if [ "$AFTER" -ne 0 ]; then
	echo "  !!! 当てても建たない"
	grep -E '^error' ../after.log | head -5
	exit 1
fi
RLIB=$(find target -name 'liblibkrimes.rlib' | head -1)
[ -n "$RLIB" ] || { echo "  !!! 成果物が無い"; exit 1; }
echo "  建った: $(wc -c < "$RLIB") bytes"
