#!/bin/sh
# cgd (NetBSD) と softraid crypto (OpenBSD) を実機で動かして性質を確かめる。
#
# 一度目の下調べで分かったこと。
#   cgd      keygen shell_cmd "path"; は構文誤り。cmd は keygen の block の
#            中に書く（man の PARAMETERS FILE: "cmd string — The command to
#            execute. Only used for the shell_cmd key generation method."）
#   softraid bioctl -s で /dev/stdin からパスフレーズを読める。-p passfile も
#            ある。鍵を手元に置かずに済む道がある
#
# 一度目は終了状態の見方も誤っていた。cmd | sed && … と書いたので判定して
# いたのは sed の終了状態で、構文誤りが出ているのに「通った」と表示していた。
# ここでは pipeline を挟まず、log をファイルに落として $? を直に見る。
set -e
say() { echo; echo "### $*"; }
W=$HOME/crypt-probe

# 走らせて、通ったかを正しく判定する。pipeline を挟まない。
run() {
	_what=$1; shift
	if "$@" > "$W/out" 2>&1; then
		echo "  [通った] $_what"
		sed 's/^/    /' "$W/out"
		return 0
	fi
	echo "  [駄目]  $_what (exit $?)"
	sed 's/^/    /' "$W/out"
	return 1
}

case $(uname) in
NetBSD|OpenBSD) ;;
*) echo "この OS は見ない"; exit 0 ;;
esac

rm -rf "$W"; mkdir -p "$W"; cd "$W"
dd if=/dev/zero of="$W/disk.img" bs=1m count=64 2>/dev/null

if [ "$(uname)" = NetBSD ]; then
	say "vnd を作る"
	run "vnconfig" vnconfig vnd0 "$W/disk.img" || true

	for form in raw base64; do
		say "shell_cmd を試す（鍵は $form）"
		if [ "$form" = raw ]; then
			printf '#!/bin/sh\ndd if=/dev/urandom bs=32 count=1 2>/dev/null\n' > "$W/keycmd"
		else
			printf '#!/bin/sh\ndd if=/dev/urandom bs=32 count=1 2>/dev/null | base64 | tr -d "\\n"\n' > "$W/keycmd"
		fi
		chmod +x "$W/keycmd"
		# 同じ鍵が二度出ないと開けないので、一度作って固定する。
		"$W/keycmd" > "$W/thekey"
		printf '#!/bin/sh\ncat %s/thekey\n' "$W" > "$W/keycmd"
		chmod +x "$W/keycmd"

		cat > "$W/p-shell" <<CONF
algorithm aes-xts;
iv-method encblkno1;
keylength 256;
verify_method none;
keygen shell_cmd {
	cmd "$W/keycmd";
};
CONF
		sed 's/^/    /' "$W/p-shell"
		if run "cgdconfig cgd0" cgdconfig cgd0 /dev/vnd0d "$W/p-shell"; then
			echo "  --- 開いた先 ---"
			ls -l /dev/cgd0d 2>&1 | sed 's/^/    /'
			# 本当に使えるか。書いて読めるか見る。
			run "dd 書き込み" dd if=/dev/urandom of=/dev/rcgd0d bs=8k count=1 || true
			cgdconfig -u cgd0 2>/dev/null || true
			echo "  ==> $form で通る"
			break
		fi
	done

	say "片付け"
	cgdconfig -u cgd0 2>/dev/null || true
	vnconfig -u vnd0 2>/dev/null || true
fi

if [ "$(uname)" = OpenBSD ]; then
	say "vnd を作って RAID の区画を切る"
	run "vnconfig" vnconfig vnd0 "$W/disk.img" || true
	printf 'y\n' | fdisk -iy vnd0 > "$W/out" 2>&1 || true
	# 区画 a を RAID 型にする。
	#
	# disklabel -E に対話入力を流す形では型が付かなかった（4.2BSD のまま
	# になり、bioctl が "invalid metadata format" で断った）。今ある label を
	# 書き出し、型だけ置き換えて -R で戻す。対話に頼らない。
	# 区画 a を新しく書く。前の回は型を直すことだけ考えて、a を作る方を
	# 落としていた。素の vnd には c しか無いので、4.2BSD を RAID に置き換える
	# 対象が存在せず、a の無い label を書き込んでいた。
	disklabel vnd0 > "$W/label" 2>/dev/null || true
	TOTAL=$(awk '$1 == "c:" { print $2 }' "$W/label")
	[ -n "$TOTAL" ] || TOTAL=131072
	ASIZE=$((TOTAL - 128))
	awk -v sz="$ASIZE" '
		/^  c:/ { print "  a: " sz " 128 RAID" }
		{ print }
	' "$W/label" > "$W/label.raid"
	echo "  --- 書き込む label ---"
	sed -n '/16 partitions/,$p' "$W/label.raid" | sed 's/^/    /'
	run "disklabel -R" disklabel -R vnd0 "$W/label.raid" || true
	echo "  --- 切った結果 ---"
	disklabel vnd0 2>&1 | tail -4 | sed 's/^/    /'

	say "stdin からパスフレーズを渡して crypto volume を作る"
	# -s は /dev/stdin から読む。確認も再入力もしない。
	# 鍵を手元に置かずに済むかどうかが、ここで決まる。
	if printf 'this-is-a-test-passphrase\n' | bioctl -s -c C -l /dev/vnd0a softraid0 > "$W/out" 2>&1; then
		echo "  [通った] bioctl -s -c C"
		sed 's/^/    /' "$W/out"
		SD=$(sed -n 's/.*\(sd[0-9]\+\).*/\1/p' "$W/out" | head -1)
		echo "  出来た volume: ${SD:-不明}"
		bioctl softraid0 2>&1 | head -8 | sed 's/^/    /'
		[ -n "$SD" ] && run "detach" bioctl -d "$SD" || true
	else
		echo "  [駄目] bioctl -s -c C (exit $?)"
		sed 's/^/    /' "$W/out"
	fi

	say "片付け"
	vnconfig -u vnd0 2>/dev/null || true
fi

cd "$HOME"; rm -rf "$W"
echo; echo "### 済"
