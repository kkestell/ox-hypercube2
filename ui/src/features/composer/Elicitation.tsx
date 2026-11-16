import { invoke } from "@tauri-apps/api/core";
import { useEffect, useRef, useState } from "react";
import type { EnumOption, Field, Item } from "../../lib/protocol";
import { showError } from "../../lib/showError";

/** What a field holds while the form is open: text, a choice's value, or
 * a multiple choice's values. */
type Value = string | string[];

function initial(field: Field): Value {
  return field.default ?? (field.type === "array" ? [] : "");
}

/** The answer a field sends, or `undefined` when it is left empty. */
function answer(value: Value | undefined) {
  return value && value.length > 0 ? value : undefined;
}

/** Why a multiple choice's selection is invalid, or `""` when it is valid.
 * An optional field left empty is omitted, so its bounds do not apply. */
function selectionError(field: Extract<Field, { type: "array" }>, count: number) {
  if (count === 0 && !field.required) return "";
  const min = Math.max(field.minItems ?? 0, 1);
  if (count < min) return `Select at least ${min}.`;
  if (field.maxItems !== undefined && count > field.maxItems) {
    return `Select at most ${field.maxItems}.`;
  }
  return "";
}

/** A form the agent asks the user to fill in, with its fields. */
export function Elicitation({
  item,
  sessionId,
}: {
  item: Extract<Item, { type: "elicitation" }>;
  sessionId: string;
}) {
  const [values, setValues] = useState<Record<string, Value>>(() =>
    Object.fromEntries(item.fields.map((field) => [field.name, initial(field)])),
  );
  const [pending, setPending] = useState(false);
  const form = useRef<HTMLFormElement>(null);

  const set = (name: string, value: Value) =>
    setValues((current) => ({ ...current, [name]: value }));

  // A multiple choice's bounds are checked with the browser's own
  // validation, on the group's first checkbox.
  useEffect(() => {
    for (const field of item.fields) {
      if (field.type !== "array") continue;
      const group = form.current?.elements.namedItem(field.name);
      const first = group instanceof RadioNodeList ? group[0] : group;
      if (first instanceof HTMLInputElement) {
        first.setCustomValidity(selectionError(field, (values[field.name] as string[]).length));
      }
    }
  }, [item.fields, values]);

  const respond = async (action: object) => {
    setPending(true);
    try {
      await invoke("respond_elicitation", { sessionId, requestId: item.request_id, action });
    } catch (e) {
      setPending(false);
      await showError(e);
    }
  };

  const submit = () => {
    const content: Record<string, unknown> = {};
    for (const field of item.fields) {
      const value = answer(values[field.name]);
      if (value !== undefined) content[field.name] = value;
    }
    respond({ action: "accept", content });
  };

  const control = (field: Field) => {
    const value = values[field.name];
    const label = field.title ?? field.name;
    switch (field.type) {
      case "string": {
        if (field.oneOf) {
          if (field.oneOf.length === 0) return null;
          return field.oneOf.map((option, index) => (
            <Choice
              key={option.const}
              type="radio"
              name={field.name}
              option={option}
              required={field.required && index === 0}
              checked={value === option.const}
              onChange={() => set(field.name, option.const)}
            />
          ));
        }
        return (
          <textarea
            className="text-field"
            name={field.name}
            aria-label={label}
            required={field.required}
            value={value as string}
            onChange={(event) => set(field.name, event.target.value)}
          />
        );
      }
      case "array": {
        // Items of a type the app does not know have no choices to show.
        const options = field.items.anyOf ?? [];
        if (options.length === 0) return null;
        const selected = value as string[];
        return options.map((option) => (
          <Choice
            key={option.const}
            type="checkbox"
            name={field.name}
            option={option}
            checked={selected.includes(option.const)}
            // The selection keeps the options' order.
            onChange={(checked) =>
              set(
                field.name,
                options
                  .map((choice) => choice.const)
                  .filter((choice) =>
                    choice === option.const ? checked : selected.includes(choice),
                  ),
              )
            }
          />
        ));
      }
      default:
        return null;
    }
  };

  return (
    <form
      ref={form}
      className="request-panel"
      onSubmit={(event) => {
        event.preventDefault();
        submit();
      }}
    >
      <div className="scroll">
        <p className="title">{item.message}</p>
        {item.fields.map((field) => {
          const input = control(field);
          if (input === null) return null;
          return (
            <fieldset key={field.name} disabled={pending}>
              <legend>{field.title ?? field.name}</legend>
              {field.description && <p>{field.description}</p>}
              {input}
            </fieldset>
          );
        })}
      </div>
      <div className="actions">
        <button
          type="button"
          className="text-button"
          disabled={pending}
          onClick={() => respond({ action: "decline" })}
        >
          Decline
        </button>
        <button type="submit" className="text-button" disabled={pending}>
          Submit
        </button>
      </div>
    </form>
  );
}

/** A radio or checkbox row: the input, the option's title, and its description. */
function Choice({
  type,
  name,
  option,
  required,
  checked,
  onChange,
}: {
  type: "radio" | "checkbox";
  name: string;
  option: EnumOption;
  required?: boolean;
  checked: boolean;
  onChange: (checked: boolean) => void;
}) {
  return (
    <label className="choice">
      <input
        type={type}
        name={name}
        value={option.const}
        required={required}
        checked={checked}
        onChange={(event) => onChange(event.target.checked)}
      />
      {option.title}
      {option.description && <span className="description">{option.description}</span>}
    </label>
  );
}
