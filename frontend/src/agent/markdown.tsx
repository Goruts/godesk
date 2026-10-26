import { useState } from "react";
import type { ReactNode } from "react";
import { writeClipboardText } from "../lib/clipboard";

/**
 * A small markdown reader for what the agent writes: headings, paragraphs,
 * lists, quotes, tables, rules, `code`, links and fenced code blocks. It is
 * not a full markdown implementation on purpose: what agents write is what
 * has to read well.
 */

type Block =
  | { kind: "code"; language: string; lines: string[] }
  | { kind: "list"; ordered: boolean; start: number; items: string[] }
  | { kind: "heading"; level: number; text: string }
  | { kind: "quote"; lines: string[] }
  | { kind: "table"; head: string[]; rows: string[][] }
  | { kind: "rule" }
  | { kind: "text"; lines: string[] };

const BULLET = /^\s*[-*+]\s+(.*)$/;
const NUMBERED = /^\s*(\d+)[.)]\s+(.*)$/;
const TABLE_ROW = /^\s*\|.*\|\s*$/;
const TABLE_RULE = /^\s*\|?\s*:?-{2,}:?\s*(\|\s*:?-{2,}:?\s*)*\|?\s*$/;

function cells(row: string): string[] {
  return row
    .trim()
    .replace(/^\||\|$/g, "")
    .split("|")
    .map((c) => c.trim());
}

export function blocks(text: string): Block[] {
  const result: Block[] = [];
  const lines = text.replace(/\r\n/g, "\n").split("\n");
  let i = 0;
  let paragraph: string[] = [];
  const flush = () => {
    if (paragraph.length) result.push({ kind: "text", lines: paragraph });
    paragraph = [];
  };

  while (i < lines.length) {
    const line = lines[i] ?? "";
    const fence = line.match(/^\s*(```+|~~~+)\s*([\w+#.-]*)/);
    if (fence) {
      flush();
      const close = fence[1] ?? "```";
      const code: string[] = [];
      i++;
      while (i < lines.length && !(lines[i] ?? "").trim().startsWith(close)) code.push(lines[i++] ?? "");
      i++; // the closing fence
      result.push({ kind: "code", language: fence[2] ?? "", lines: code });
      continue;
    }
    const heading = line.match(/^(#{1,6})\s+(.*?)\s*#*\s*$/);
    if (heading) {
      flush();
      result.push({ kind: "heading", level: heading[1]?.length ?? 1, text: heading[2] ?? "" });
      i++;
      continue;
    }
    if (/^\s*([-*_])(\s*\1){2,}\s*$/.test(line)) {
      flush();
      result.push({ kind: "rule" });
      i++;
      continue;
    }
    if (TABLE_ROW.test(line) && TABLE_RULE.test(lines[i + 1] ?? "")) {
      flush();
      const head = cells(line);
      const rows: string[][] = [];
      i += 2;
      while (i < lines.length && TABLE_ROW.test(lines[i] ?? "")) rows.push(cells(lines[i++] ?? ""));
      result.push({ kind: "table", head, rows });
      continue;
    }
    if (/^\s*>/.test(line)) {
      flush();
      const quote: string[] = [];
      while (i < lines.length && /^\s*>/.test(lines[i] ?? "")) quote.push((lines[i++] ?? "").replace(/^\s*>\s?/, ""));
      result.push({ kind: "quote", lines: quote });
      continue;
    }
    const bullet = line.match(BULLET);
    const numbered = line.match(NUMBERED);
    if (bullet || numbered) {
      flush();
      const ordered = !bullet;
      const items: string[] = [];
      while (i < lines.length) {
        const current = lines[i] ?? "";
        const match = current.match(ordered ? NUMBERED : BULLET);
        if (match) {
          items.push((ordered ? match[2] : match[1]) ?? "");
        } else if (/^\s{2,}\S/.test(current) && items.length) {
          // A wrapped line of the item above.
          items[items.length - 1] += ` ${current.trim()}`;
        } else {
          break;
        }
        i++;
      }
      result.push({ kind: "list", ordered, start: numbered ? Number(numbered[1]) : 1, items });
      continue;
    }
    if (!line.trim()) {
      flush();
      i++;
      continue;
    }
    paragraph.push(line);
    i++;
  }
  flush();
  return result;
}

/** `code`, **bold**, *italic*, ~~struck~~ and [links](…) inside a line. */
function inline(text: string, keyPrefix: string): ReactNode[] {
  const nodes: ReactNode[] = [];
  const pattern = /(`[^`]+`|\*\*[^*]+\*\*|__[^_]+__|\*[^*\s][^*]*\*|~~[^~]+~~|\[[^\]]+\]\([^)\s]+\))/g;
  let last = 0;
  let match: RegExpExecArray | null;
  let index = 0;
  while ((match = pattern.exec(text)) !== null) {
    if (match.index > last) nodes.push(text.slice(last, match.index));
    const token = match[0];
    const key = `${keyPrefix}-${index++}`;
    if (token.startsWith("`")) {
      nodes.push(<code key={key}>{token.slice(1, -1)}</code>);
    } else if (token.startsWith("**") || token.startsWith("__")) {
      nodes.push(<strong key={key}>{inline(token.slice(2, -2), key)}</strong>);
    } else if (token.startsWith("~~")) {
      nodes.push(<s key={key}>{token.slice(2, -2)}</s>);
    } else if (token.startsWith("[")) {
      const link = token.match(/^\[([^\]]+)\]\(([^)\s]+)\)$/);
      nodes.push(
        <span key={key} className="md-link" title={link?.[2]}>
          {link?.[1]}
        </span>,
      );
    } else {
      nodes.push(<em key={key}>{token.slice(1, -1)}</em>);
    }
    last = match.index + token.length;
  }
  if (last < text.length) nodes.push(text.slice(last));
  return nodes;
}

/** A code block: its language, a copy button and the code. */
export function CodeBlock({ code, language }: { code: string; language: string }) {
  const [copied, setCopied] = useState(false);
  const copy = () => {
    void writeClipboardText(code).then(() => {
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1400);
    });
  };
  return (
    <div className="md-code">
      <div className="md-code-bar">
        <span className="md-code-lang">{language || "text"}</span>
        <button type="button" className="md-code-copy" onClick={copy}>
          {copied ? "Copied" : "Copy"}
        </button>
      </div>
      <pre>
        <code>{code}</code>
      </pre>
    </div>
  );
}

export function Markdown({ text }: { text: string }) {
  return (
    <div className="md">
      {blocks(text).map((block, i) => {
        const key = `b${i}`;
        switch (block.kind) {
          case "code":
            return <CodeBlock key={key} code={block.lines.join("\n")} language={block.language} />;
          case "heading": {
            const level = Math.min(block.level + 2, 6);
            const Tag = `h${level}` as "h3";
            return (
              <Tag key={key} className="md-heading" data-level={block.level}>
                {inline(block.text, key)}
              </Tag>
            );
          }
          case "rule":
            return <hr key={key} className="md-rule" />;
          case "quote":
            return (
              <blockquote key={key} className="md-quote">
                {block.lines.map((line, j) => (
                  <p key={`${key}-${j}`}>{inline(line, `${key}-${j}`)}</p>
                ))}
              </blockquote>
            );
          case "table":
            return (
              <div key={key} className="md-table-wrap">
                <table className="md-table">
                  <thead>
                    <tr>
                      {block.head.map((cell, j) => (
                        <th key={`${key}-h${j}`}>{inline(cell, `${key}-h${j}`)}</th>
                      ))}
                    </tr>
                  </thead>
                  <tbody>
                    {block.rows.map((row, r) => (
                      <tr key={`${key}-r${r}`}>
                        {row.map((cell, j) => (
                          <td key={`${key}-r${r}-${j}`}>{inline(cell, `${key}-r${r}-${j}`)}</td>
                        ))}
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            );
          case "list": {
            const items = block.items.map((item, j) => <li key={`${key}-${j}`}>{inline(item, `${key}-${j}`)}</li>);
            return block.ordered ? (
              <ol key={key} className="md-list" start={block.start}>
                {items}
              </ol>
            ) : (
              <ul key={key} className="md-list">
                {items}
              </ul>
            );
          }
          default:
            return (
              <p key={key} className="md-paragraph">
                {block.lines.map((line, j) => (
                  <span key={`${key}-${j}`}>
                    {inline(line, `${key}-${j}`)}
                    {j < block.lines.length - 1 ? <br /> : null}
                  </span>
                ))}
              </p>
            );
        }
      })}
    </div>
  );
}
