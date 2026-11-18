# telora 的开发与本地集成命令。

# 显示可用命令。
default:
    @just --list

# 把编辑器查询同步进本机 Helix（HELIX_DIR 可覆盖配置目录）。
install-into-helix:
    #!/usr/bin/env bash
    set -euo pipefail
    HELIX_DIR="${HELIX_DIR:-$HOME/.config/helix}"
    RUNTIME="$HELIX_DIR/runtime"
    FROM="tree-sitter-telora/queries"
    TO="$RUNTIME/queries/telora"
    [ -d "$RUNTIME" ] || { printf '找不到 Helix runtime：%s\n' "$RUNTIME" >&2; exit 1; }
    [ -d "$FROM" ] || { printf '找不到查询目录：%s\n' "$FROM" >&2; exit 1; }
    mkdir -p "$TO"
    changed=0
    for name in highlights indents textobjects; do
      file="$FROM/$name.scm"
      [ -f "$file" ] || continue
      cmp -s "$file" "$TO/$name.scm" 2>/dev/null && continue
      [ -f "$TO/$name.scm" ] && cp "$TO/$name.scm" "$TO/$name.scm.bak"
      cp "$file" "$TO/$name.scm"
      changed=1
      printf '更新 telora/%s.scm\n' "$name"
    done
    [ "$changed" -eq 0 ] && printf 'telora 查询已是最新\n'
    [ -f "$RUNTIME/grammars/telora.so" ] || printf '提示：缺少 %s/grammars/telora.so，可运行 hx --grammar build\n' "$RUNTIME"
    grep -q 'name = "telora"' "$HELIX_DIR/languages.toml" 2>/dev/null || printf '提示：%s/languages.toml 缺少 telora 的语言配置\n' "$HELIX_DIR"
    printf '完成：查询已同步到 %s，重启 hx 后生效\n' "$TO"
