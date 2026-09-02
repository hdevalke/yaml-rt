export const READ_COMMANDS = new Set(["query", "get", "test"]);

function lines(value) {
  return value.split(/\r?\n/);
}

export function lineDiff(before, after) {
  const left = lines(before);
  const right = lines(after);
  if (left.length * right.length > 250000) {
    let prefix = 0;
    while (prefix < left.length && prefix < right.length && left[prefix] === right[prefix]) prefix++;
    let suffix = 0;
    while (suffix < left.length - prefix && suffix < right.length - prefix
      && left[left.length - 1 - suffix] === right[right.length - 1 - suffix]) suffix++;
    return {
      changedLines: Array.from(
        { length: Math.max(0, right.length - suffix - prefix) },
        (_, index) => prefix + index + 1,
      ),
      deletions: left.length - suffix > prefix
        ? [{ line: Math.min(prefix + 1, right.length), removedLines: left.slice(prefix, left.length - suffix) }]
        : [],
    };
  }

  const lengths = Array.from({ length: left.length + 1 }, () => new Uint32Array(right.length + 1));
  for (let i = left.length - 1; i >= 0; i--) {
    for (let j = right.length - 1; j >= 0; j--) {
      lengths[i][j] = left[i] === right[j]
        ? lengths[i + 1][j + 1] + 1
        : Math.max(lengths[i + 1][j], lengths[i][j + 1]);
    }
  }

  const changedLines = [];
  const deletions = [];
  let removedLines = [];
  let i = 0;
  let j = 0;
  const flushDeletions = () => {
    if (!removedLines.length) return;
    deletions.push({ line: Math.min(j + 1, right.length), removedLines });
    removedLines = [];
  };
  while (i < left.length && j < right.length) {
    if (left[i] === right[j]) {
      flushDeletions();
      i++;
      j++;
    } else if (lengths[i + 1][j] >= lengths[i][j + 1]) {
      removedLines.push(left[i++]);
    } else {
      flushDeletions();
      changedLines.push(++j);
    }
  }
  while (i < left.length) removedLines.push(left[i++]);
  flushDeletions();
  while (j < right.length) changedLines.push(++j);
  return { changedLines, deletions };
}

export async function copyText(value, { clipboard, fallback }) {
  if (clipboard?.writeText) {
    try {
      await clipboard.writeText(value);
      return true;
    } catch {
      // Insecure HTTP origins commonly expose the API but reject the write.
    }
  }
  try {
    return Boolean(fallback(value));
  } catch {
    return false;
  }
}

export function resultPresentation(result, command, source) {
  if (command === "validate") {
    return {
      content: result.ok ? result.command_output : "",
      title: "Validation Result",
      highlightChanges: false,
      showMatchCount: false,
      showCopyResult: false,
    };
  }
  if (result.ok && READ_COMMANDS.has(command)) {
    return {
      content: result.command_output,
      title: "Command Output",
      highlightChanges: false,
      showMatchCount: true,
      showCopyResult: true,
    };
  }
  if (result.ok) {
    return {
      content: result.output_yaml,
      title: "Result YAML",
      highlightChanges: true,
      showMatchCount: false,
      showCopyResult: true,
    };
  }
  return {
    content: result.error_source === "application" ? source : "",
    title: "Result YAML",
    highlightChanges: false,
    showMatchCount: false,
    showCopyResult: true,
  };
}
