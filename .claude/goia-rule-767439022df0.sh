: goia-rule-guard
die() { printf '%s\n' "Goia rule guard: $1, so the call is refused." >&2; exit 2; }
p=$(cat) || die 'the tool call could not be read'
c=$(printf '%s' "$p" | sed -nE 's/^.*"command":"(([^"\\]|\\.)*)".*$/\1/p') || die 'the command could not be extracted'
[ -n "$c" ] || c=$p
c=$(printf '%s' "$c" | sed -E 's/\\\\/@@BS@@/g;s/@@BS@@(\\r)?\\n//g;s/\\"/"/g;s/\\\//\//g;s/\\t/ /g;s/\\r//g;s/\\n/\
/g;s/@@BS@@/\\/g') || die 'the command could not be decoded'
m=$(printf '%s\n' "$c" | sed -E 's/[Gg][Ii][Tt]\.([Ee][Xx][Ee]|[Cc][Mm][Dd])/git/g;s/[Gg][Hh]\.([Ee][Xx][Ee]|[Cc][Mm][Dd])/gh/g') || die 'the command could not be normalised'
n=$(printf '%s\n' "$m" | sed -E 's/git([[:space:]]+(-[Cc][[:space:]]+("[^"]*"|'\''[^'\'']*'\''|\\.|[^[:space:]"'\'';&|<>()\\])+|--(git-dir|work-tree|namespace|config-env|attr-source)[[:space:]]+("[^"]*"|'\''[^'\'']*'\''|\\.|[^[:space:]"'\'';&|<>()\\])+|--[A-Za-z][A-Za-z-]*(=("[^"]*"|'\''[^'\'']*'\''|\\.|[^[:space:]"'\'';&|<>()\\])+)?|-[pPvh]))+([[:space:]]|$)/git /g;s/gh([[:space:]]+(-R[[:space:]]*("[^"]*"|'\''[^'\'']*'\''|\\.|[^[:space:]"'\'';&|<>()\\])+|--repo(=("[^"]*"|'\''[^'\'']*'\''|\\.|[^[:space:]"'\'';&|<>()\\])+|[[:space:]]+("[^"]*"|'\''[^'\'']*'\''|\\.|[^[:space:]"'\'';&|<>()\\])+)))+([[:space:]]|$)/gh /g') || die 'the command could not be normalised'
u=$(printf '%s\n' "$m" | sed -E 's/\$(['\''"])/\1/g;s/["'\''`\\]//g') || die 'the command could not be unquoted'
x=$(printf '%s\n' "$u" | sed -E 's/git([[:space:]]+(-[Cc][[:space:]]+("[^"]*"|'\''[^'\'']*'\''|\\.|[^[:space:]"'\'';&|<>()\\])+|--(git-dir|work-tree|namespace|config-env|attr-source)[[:space:]]+("[^"]*"|'\''[^'\'']*'\''|\\.|[^[:space:]"'\'';&|<>()\\])+|--[A-Za-z][A-Za-z-]*(=("[^"]*"|'\''[^'\'']*'\''|\\.|[^[:space:]"'\'';&|<>()\\])+)?|-[pPvh]))+([[:space:]]|$)/git /g;s/gh([[:space:]]+(-R[[:space:]]*("[^"]*"|'\''[^'\'']*'\''|\\.|[^[:space:]"'\'';&|<>()\\])+|--repo(=("[^"]*"|'\''[^'\'']*'\''|\\.|[^[:space:]"'\'';&|<>()\\])+|[[:space:]]+("[^"]*"|'\''[^'\'']*'\''|\\.|[^[:space:]"'\'';&|<>()\\])+)))+([[:space:]]|$)/gh /g') || die 'the command could not be normalised'
q=$(printf '%s\n' "$n" | sed -E 's/\$(['\''"])/\1/g;s/["'\''`\\]//g') || die 'the command could not be unquoted'
printf '%s\n' "$m" | grep -Eq '(^|[^[:alnum:]_.-])(git|gh)[[:space:]][^;&|]*\$'\''[^'\'']*\\[xXuU0-7]'; a=$?
[ "$a" -le 1 ] || die 'the command could not be inspected'
[ "$a" -eq 1 ] || die 'a git or gh argument written as an ANSI-C string with escapes cannot be read'
printf '%s\n%s\n' "$n" "$x" | grep -Eq '(^|[^[:alnum:]_.-])(git|gh)[[:space:]]+[-$`\\'"'"'"]'; g=$?
[ "$g" -le 1 ] || die 'the command could not be inspected'
s=; if [ "$g" -eq 0 ]; then s=$(printf '%s\n%s\n%s\n' "$n" "$x" "$q" | while IFS= read -r l; do r=$l; while :; do printf 'git %s\ngh %s\n' "$r" "$r"; case $r in *[[:space:]]*) r=${r#*[[:space:]]};; *) break;; esac; done; done) || die 'the command could not be searched'; fi
printf '%s\n%s\n%s\n%s\n%s\n%s\n%s\n' "$c" "$m" "$n" "$s" "$q" "$u" "$x" | grep -Eq -- 'git[[:space:]]+push|gh[[:space:]]+pr[[:space:]]+create'; rc=$?
[ "$rc" -eq 1 ] && exit 0
[ "$rc" -eq 0 ] && { printf '%s\n' 'Blocked by a Goia rule: Do not push or open a pull request until I explicitly say so.' >&2; exit 2; }
die 'the pattern could not be evaluated'
