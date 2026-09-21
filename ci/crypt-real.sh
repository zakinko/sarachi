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
*)
	echo "### $(uname) の暗号層はまだ実装していない。何もしない"
	exit 0
	;;
esac
trap cleanup EXIT

echo "### $DEV で暗号層を試す"
SARACHI_TEST_DEVICE=$DEV cargo test -p sarachi-recovery -- --ignored --nocapture
