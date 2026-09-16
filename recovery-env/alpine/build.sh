#!/bin/sh
# 回復環境の initramfs を組む。Alpine の上で走らせる。
#
# 全標的で同じ実行体を使う。書き戻す先の OS が何であってもこれ一つで足りるのは、
# 回復環境がやるのは署名検証・消去・取得・書き込みだけで、対象 OS の中身を
# 知る必要がないため。Linux を土台に選んだのはドライバとファームウェアの
# 網羅性のためで、無線が動かなければ遠隔からの回復は成立しない。
set -eu

OUT=${OUT:-$HOME/recovery-build}
STAGE=$OUT/root
HERE=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
KVER=${KVER:-$(ls /lib/modules | head -1)}

# 入れるモジュールはここで明示する。全部入れると 22MB になるが、
# 挙げたものだけなら一桁小さい。回復環境は小さいほど RAM に載せやすく、
# 起動も速いので、増やす時は必ず理由を書くこと。
#
# af_packet は DHCP に要る。udhcpc は生パケットを使うので、これが無いと
# socket(AF_PACKET) が「Address family not supported」で失敗する。
# ネットワークが上がらないと再導入が成立しないので、外せない。
MODULES="
virtio_blk
virtio_net
nvme
dm-crypt
ext4
vfat
af_packet
"

rm -rf "$STAGE"
mkdir -p "$STAGE"/bin "$STAGE"/proc "$STAGE"/sys "$STAGE"/dev "$STAGE"/run "$STAGE"/etc "$OUT"

# busybox-static を使うのは、動的リンクだと libc を連れてくる必要があり
# 下限が測りにくくなるため。まずここを底として、足した分だけ測る。
cp /bin/busybox.static "$STAGE/bin/busybox"

# /bin/sh は init の shebang が解決される時点で既に無いといけない。
# busybox --install は init の中で走るので間に合わず、ここで張らないと
# カーネルは /init を ENOENT で見失い "No working init found" で panic する。
ln -s busybox "$STAGE/bin/sh"

install -m 0755 "$HERE/init" "$STAGE/init"

mkdir -p "$STAGE/usr/share/udhcpc" "$STAGE/etc"
install -m 0755 "$HERE/udhcpc.script" "$STAGE/usr/share/udhcpc/default.script"

# 我々の実行体。静的リンクなので libc を連れて行かなくてよい。
# BIN で場所を指せる。無ければ busybox だけの骨格として組む。
BIN=${BIN:-$OUT/unix-mdm-recovery}
if [ -f "$BIN" ]; then
    install -m 0755 "$BIN" "$STAGE/bin/unix-mdm-recovery"
    printf 'recovery  : %s\n' "$(du -h "$BIN" | cut -f1)"
fi

# 依存はビルド時に解決して順序を固定する。実行時は insmod を並べるだけに
# なるので、initramfs に depmod も modules.dep も要らず、挙動も決定的になる。
: > "$STAGE/modules.load"
for m in $MODULES; do
    modprobe --show-depends -S "$KVER" "$m" 2>/dev/null | while read -r verb path _; do
        [ "$verb" = insmod ] || continue
        rel=${path#/lib/modules/$KVER/}
        dest=$STAGE/lib/modules/${rel%.gz}
        mkdir -p "$(dirname "$dest")"
        # busybox の insmod は圧縮モジュールを読めないので展開して置く。
        case "$path" in
            *.gz) gzip -dc "$path" > "$dest" ;;
            *)    cp "$path" "$dest" ;;
        esac
        grep -qxF "/lib/modules/${rel%.gz}" "$STAGE/modules.load" || \
            echo "/lib/modules/${rel%.gz}" >> "$STAGE/modules.load"
    done
done

( cd "$STAGE" && find . | cpio -o -H newc --quiet ) | gzip -9 > "$OUT/initramfs.gz"

printf 'initramfs : %s\n' "$(du -h "$OUT/initramfs.gz" | cut -f1)"
printf '  module  : %s 個 / %s\n' \
    "$(wc -l < "$STAGE/modules.load" | tr -d ' ')" \
    "$(du -sh "$STAGE/lib/modules" 2>/dev/null | cut -f1)"

for k in /boot/vmlinuz-virt /boot/vmlinuz-lts /boot/vmlinuz; do
    [ -f "$k" ] && { cp "$k" "$OUT/kernel"; printf 'kernel    : %s (%s)\n' "$k" "$(du -h "$k" | cut -f1)"; break; }
done
