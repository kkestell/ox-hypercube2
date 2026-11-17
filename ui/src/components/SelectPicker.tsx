import { ChevronDown } from "lucide-react";
import type { SelectGroup, SelectOption } from "../lib/protocol";

/** A styled native select, with its platform menu and keyboard behavior. */
export function SelectPicker({
  label,
  value,
  options,
  disabled = false,
  onChange,
}: {
  label: string;
  value: string;
  options: SelectOption[] | SelectGroup[];
  disabled?: boolean;
  onChange: (value: string) => void;
}) {
  const choice = (entry: SelectOption) => (
    <option key={entry.value} value={entry.value}>
      {entry.name}
    </option>
  );

  return (
    <span className="select-picker">
      <select
        aria-label={label}
        title={label}
        value={value}
        disabled={disabled}
        onChange={(event) => onChange(event.target.value)}
      >
        {options.map((entry) =>
          "group" in entry ? (
            <optgroup key={entry.group} label={entry.name}>
              {entry.options.map(choice)}
            </optgroup>
          ) : (
            choice(entry)
          ),
        )}
      </select>
      <ChevronDown aria-hidden="true" />
    </span>
  );
}
