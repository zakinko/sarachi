#!/bin/sh
# ci/ の shell script を、走らせる前に機械的に見る。
#
# ここにある script は BSD の VM の中でしか走らず、一回が 30 分から一時間
# かかる。書き間違いに気づくのがその後になると、一日で何時間も溶ける。
# 実際に溶かしたので、静的に見える分はここで見る。
set -e

# 探す文字そのものは書かない。書くとこの script 自身が引っかかる。
BQ=$(printf '\140')

fail=0

for f in ci/*.sh; do
	# 文法。引用の閉じ忘れはここで出る。
	if ! sh -n "$f"; then
		echo "$f: 文法が通らない" >&2
		fail=1
	fi

	# command substitution は $(...) で書く決まりなので、これが在るのは
	# 十中八九 comment の中の引用で、しかもそれが一番危ない。
	#
	# 変数を展開させる heredoc（引用しない <<EOF）の中身は shell に読まれる。
	# comment のつもりで書いても、そこは command substitution として実行
	# される。config.toml を作る heredoc で実際に二度やった。一度目は失敗
	# log を貼った所で rustc -vV が静かに走り、その行が空になった。二度目は
	# 引用符と並んで Syntax error になり、VM を起こして一時間走らせた末に
	# 落ちた。
	if grep -n "$BQ" "$f" >/dev/null 2>&1; then
		echo "$f: command substitution は \$(...) で書く。" >&2
		echo "  heredoc の中なら comment のつもりでも実行される:" >&2
		grep -n "$BQ" "$f" | sed 's/^/    /' >&2
		fail=1
	fi
done

[ "$fail" -eq 0 ] || exit 1
echo "ci/*.sh: 文法と引用、問題なし"
