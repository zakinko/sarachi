#!/bin/sh
# 暗号層の破壊的な試験を、その platform の実機で走らせる。
#
# crypt.rs の試験は既定では走らない。デバイスを壊すので、うっかり動かない
# ようにしてある。ここでそのデバイスを用意して明示的に呼ぶ。
#
# 消去は設計の土台で、しかも「消えたこと」は目で見て分からない。鍵を破棄した
# 後に開けてしまう退行が入っても、他の試験は全て通ったままになる。だから
# 覚えている人が思い出したときに走らせるのではなく、CI に走らせる。
set -e

W=$(pwd)/.ci-crypt
rm -rf "$W"; mkdir -p "$W"
dd if=/dev/zero of="$W/disk.img" bs=1024k count=64 2>/dev/null

case $(uname) in
FreeBSD)
	MD=$(mdconfig -a -t vnode -f "$W/disk.img")
	DEV=/dev/$MD
	cleanup() { mdconfig -d -u "$MD" 2>/dev/null || true; }
	;;
Linux)
	DEV=$(losetup --find --show "$W/disk.img")
	cleanup() { losetup -d "$DEV" 2>/dev/null || true; }
	;;
DragonFly)
	# DragonFly は geli ではなく LUKS。base に cryptsetup と dmsetup が在り、
	# dm_target_crypt が読み込める（実機で確かめた）。dm は自動では上がって
	# いないので明示する。
	kldload dm 2>/dev/null || true
	kldload dm_target_crypt 2>/dev/null || true
	vnconfig vnd0 "$W/disk.img"
	DEV=/dev/vnd0
	cleanup() { vnconfig -u vnd0 2>/dev/null || true; }
	;;
NetBSD)
	vnconfig vnd0 "$W/disk.img"
	DEV=/dev/vnd0d
	cleanup() {
		cgdconfig -u cgd0 2>/dev/null || true
		vnconfig -u vnd0 2>/dev/null || true
	}
	;;
OpenBSD)
	vnconfig vnd0 "$W/disk.img"
	# softraid に渡す区画は型が RAID でなければならない。4.2BSD のままだと
	# bioctl が invalid metadata format で断る。素の vnd には c しか無いので、
	# a を作って型を付ける。
	disklabel vnd0 > "$W/label" 2>/dev/null || true
	TOTAL=$(awk '$1 == "c:" { print $2 }' "$W/label")
	[ -n "$TOTAL" ] || TOTAL=131072
	awk -v sz="$((TOTAL - 128))" '
		/^  c:/ { print "  a: " sz " 128 RAID" }
		{ print }
	' "$W/label" > "$W/label.raid"
	disklabel -R vnd0 "$W/label.raid"
	DEV=/dev/vnd0a
	cleanup() { vnconfig -u vnd0 2>/dev/null || true; }
	;;
*)
	echo "### $(uname) の暗号層はまだ実装していない。何もしない"
	exit 0
	;;
esac
trap cleanup EXIT

echo "### $DEV で暗号層を試す"
SARACHI_TEST_DEVICE=$DEV cargo test -p sarachi-recovery -- --ignored --nocapture
