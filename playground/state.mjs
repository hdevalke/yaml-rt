export const READ_COMMANDS = new Set(["query", "get", "test"]);

export function resultPresentation(result, command, source) {
  if (result.ok && READ_COMMANDS.has(command)) {
    return {
      content: result.command_output,
      title: "Command Output",
      highlightChanges: false,
      showMatchCount: true,
    };
  }
  if (result.ok) {
    return {
      content: result.output_yaml,
      title: "Result YAML",
      highlightChanges: true,
      showMatchCount: false,
    };
  }
  return {
    content: result.error_source === "application" ? source : "",
    title: "Result YAML",
    highlightChanges: false,
    showMatchCount: false,
  };
}
