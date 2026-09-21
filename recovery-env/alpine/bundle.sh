#!/bin/sh
# 配布する三つの像を作る。Alpine の上で走らせる。
#
# ESP も回復領域も root も、どれもファイルシステムの像として配る。
# こうすると回復環境に mkfs を持ち込まずに済み、OS ごとに mkfs.vfat /
# mkfs.ext4 / newfs / newfs_msdos を揃える必要がなくなる。検証して書けば
# そのまま使える。
#
# カーネルと initramfs は ESP に置く。systemd-boot は FAT しか読めないため。
# 回復領域にはファームウェアを置く。initramfs に抱えると RAM に常駐する分だけ
# 効いてくるが、カーネルは必要になった時に読むので、ディスクに置けば足りる。
set -eu

OUT=${OUT:-$HOME/recovery-build}
HERE=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
DIST=${DIST:-$OUT/dist}
ESP_MB=${ESP_MB:-256}
REC_MB=${REC_MB:-128}

mkdir -p "$DIST"
sh "$HERE/build.sh" >/dev/null

echo "### ESP の像（ローダ + カーネル + initramfs）"
E=$DIST/esp.img
rm -f "$E"
dd if=/dev/zero of="$E" bs=1M count="$ESP_MB" status=none
mkfs.vfat -F 32 -n ESP "$E" >/dev/null
mmd -i "$E" ::/EFI ::/EFI/BOOT ::/loader ::/loader/entries
mcopy -i "$E" /usr/lib/systemd/boot/efi/systemd-bootaa64.efi ::/EFI/BOOT/BOOTAA64.EFI
mcopy -i "$E" "$OUT/kernel" ::/vmlinuz
mcopy -i "$E" "$OUT/initramfs.gz" ::/initramfs.gz

# timeout を 0 にしないのは、導入された機体では回復環境が既定であっては
# 困るため。実 OS のエントリが増えたときに選べる余地を残す。
printf 'default recovery\ntimeout 3\nconsole-mode max\n' > "$OUT/loader.conf"
mcopy -i "$E" "$OUT/loader.conf" ::/loader/loader.conf
printf 'title sarachi recovery\nlinux /vmlinuz\ninitrd /initramfs.gz\noptions console=ttyAMA0 sarachi.net\n' \
    > "$OUT/recovery.conf"
mcopy -i "$E" "$OUT/recovery.conf" ::/loader/entries/recovery.conf
printf '  %s (%s MiB)\n' "$E" "$ESP_MB"

echo "### 回復領域の像（ファームウェア）"
R=$DIST/recovery.img
rm -f "$R"
dd if=/dev/zero of="$R" bs=1M count="$REC_MB" status=none
mkfs.ext4 -q -L SARACHI-RECOVERY "$R"
W=$(mktemp -d)
sudo mount -o loop "$R" "$W"
sudo mkdir -p "$W/firmware" "$W/etc"
# 実機向けには絞ったファームウェア一式をここへ置く（188MiB 程度）。
# 試験ではそこまで要らないので、置き場だけ作って目印を入れる。
echo "sarachi recovery partition 2026-09-17" | sudo tee "$W/etc/sarachi-recovery" >/dev/null
if [ -d /lib/firmware ] && [ "${WITH_FIRMWARE:-0}" = 1 ]; then
    sudo cp -a /lib/firmware/. "$W/firmware/" 2>/dev/null || true
fi
sync; sudo umount "$W"; rmdir "$W"
printf '  %s (%s MiB)\n' "$R" "$REC_MB"

echo "### 出来た像"
ls -la "$DIST"/*.img | sed 's/^/  /'
