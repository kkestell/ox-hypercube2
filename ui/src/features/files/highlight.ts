import { toJsxRuntime } from "hast-util-to-jsx-runtime";
import { common, createLowlight } from "lowlight";
import { Fragment, jsx, jsxs } from "react/jsx-runtime";

const lowlight = createLowlight(common);

export function highlightFile(path: string, text: string) {
  const name = path.split("/").at(-1)!.toLowerCase();
  const extension = name.includes(".") ? name.split(".").at(-1)! : "";
  const language = [extension, name].find((candidate) => lowlight.registered(candidate));
  if (!language) return text;
  // TODO: Large files can stall the UI while highlighting and rendering synchronously.
  return toJsxRuntime(lowlight.highlight(language, text), { Fragment, jsx, jsxs });
}
