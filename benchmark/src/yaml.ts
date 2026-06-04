interface YamlLine {
  indent: number;
  text: string;
  lineNumber: number;
}

export function parseYaml(source: string): unknown {
  const lines = normalizeLines(source);
  let index = 0;

  function current(): YamlLine | undefined {
    return lines[index];
  }

  function parseBlock(indent: number): unknown {
    const line = current();
    if (!line || line.indent < indent) {
      return {};
    }

    if (line.text.startsWith("- ") && line.indent === indent) {
      return parseArray(indent);
    }

    return parseObject(indent);
  }

  function parseArray(indent: number): unknown[] {
    const values: unknown[] = [];

    while (current() && current()!.indent === indent && current()!.text.startsWith("- ")) {
      const line = current()!;
      const rest = line.text.slice(2).trim();
      index += 1;

      if (rest.length === 0) {
        values.push(parseBlock(nextIndent(indent)));
        continue;
      }

      const keyValue = splitKeyValue(rest);
      if (keyValue) {
        const item: Record<string, unknown> = {};
        assignKeyValue(item, keyValue.key, keyValue.value, nextIndent(indent));

        while (current() && current()!.indent > indent) {
          const childIndent = current()!.indent;
          const child = parseObject(childIndent);
          Object.assign(item, child);
        }

        values.push(item);
        continue;
      }

      values.push(parseScalar(rest));
    }

    return values;
  }

  function parseObject(indent: number): Record<string, unknown> {
    const object: Record<string, unknown> = {};

    while (current() && current()!.indent === indent && !current()!.text.startsWith("- ")) {
      const line = current()!;
      const keyValue = splitKeyValue(line.text);
      if (!keyValue) {
        throw new Error(`Invalid YAML mapping at line ${line.lineNumber}: ${line.text}`);
      }

      index += 1;
      assignKeyValue(object, keyValue.key, keyValue.value, nextIndent(indent));
    }

    return object;
  }

  function assignKeyValue(target: Record<string, unknown>, key: string, rawValue: string, childIndent: number): void {
    if (rawValue.length === 0) {
      target[key] = parseBlock(childIndent);
      return;
    }

    target[key] = parseScalar(rawValue);
  }

  function nextIndent(parentIndent: number): number {
    const line = current();
    if (!line || line.indent <= parentIndent) {
      return parentIndent + 2;
    }
    return line.indent;
  }

  return parseBlock(lines[0]?.indent ?? 0);
}

function normalizeLines(source: string): YamlLine[] {
  return source
    .replace(/\r\n/g, "\n")
    .split("\n")
    .map((raw, rawIndex) => {
      const withoutComment = stripComment(raw);
      const text = withoutComment.trimEnd();
      return {
        indent: text.length - text.trimStart().length,
        text: text.trimStart(),
        lineNumber: rawIndex + 1,
      };
    })
    .filter((line) => line.text.length > 0);
}

function stripComment(line: string): string {
  let quote: string | null = null;
  for (let i = 0; i < line.length; i += 1) {
    const char = line[i];
    if ((char === "'" || char === '"') && line[i - 1] !== "\\") {
      quote = quote === char ? null : quote ?? char;
    }
    if (char === "#" && quote === null && (i === 0 || /\s/.test(line[i - 1]))) {
      return line.slice(0, i);
    }
  }
  return line;
}

function splitKeyValue(text: string): { key: string; value: string } | null {
  let quote: string | null = null;

  for (let i = 0; i < text.length; i += 1) {
    const char = text[i];
    if ((char === "'" || char === '"') && text[i - 1] !== "\\") {
      quote = quote === char ? null : quote ?? char;
    }
    if (char === ":" && quote === null) {
      const key = text.slice(0, i).trim();
      const value = text.slice(i + 1).trim();
      if (!key) {
        throw new Error(`Invalid YAML key in line: ${text}`);
      }
      return { key, value };
    }
  }

  return null;
}

function parseScalar(value: string): unknown {
  const trimmed = value.trim();
  if (trimmed === "true") return true;
  if (trimmed === "false") return false;
  if (trimmed === "null" || trimmed === "~") return null;
  if (/^-?\d+$/.test(trimmed)) return Number.parseInt(trimmed, 10);
  if (/^-?\d+\.\d+$/.test(trimmed)) return Number.parseFloat(trimmed);
  if (trimmed.startsWith('"') && trimmed.endsWith('"')) {
    return trimmed.slice(1, -1).replace(/\\"/g, '"');
  }
  if (trimmed.startsWith("'") && trimmed.endsWith("'")) {
    return trimmed.slice(1, -1).replace(/\\'/g, "'");
  }
  if (trimmed.startsWith("[") && trimmed.endsWith("]")) {
    const inner = trimmed.slice(1, -1).trim();
    if (!inner) return [];
    return inner.split(",").map((part) => parseScalar(part.trim()));
  }
  return trimmed;
}
