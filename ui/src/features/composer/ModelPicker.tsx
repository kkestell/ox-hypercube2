import { ChevronDown, Search } from "lucide-react";
import { type CSSProperties, type KeyboardEvent, useEffect, useRef, useState } from "react";
import type { ConfigOption, SelectOption } from "../../lib/protocol";

export type SelectConfig = Extract<ConfigOption, { type: "select" }>;

/** A select option's choices, with its groups flattened. */
function choices(option: SelectConfig): SelectOption[] {
  return option.options.flatMap((entry) => ("group" in entry ? entry.options : [entry]));
}

/**
 * The session's model and effort: a button showing both, which opens a menu
 * to search for and choose a model and to set the effort. The menu stays open
 * after a model is chosen, so the effort can be set next. A model or effort
 * level shows as soon as it is chosen, until the agent reports it or refuses
 * it.
 */
export function ModelPicker({
  model,
  effort,
  onChange,
}: {
  model: SelectConfig;
  effort: SelectConfig | undefined;
  /** Sets `option` to `value` and resolves to whether the agent accepted it. */
  onChange: (option: SelectConfig, value: string) => Promise<boolean>;
}) {
  const search = useRef<HTMLInputElement>(null);
  const list = useRef<HTMLDivElement>(null);
  const [query, setQuery] = useState("");
  const [active, setActive] = useState(0);
  const [selectedModel, setModel] = useChosen(model, onChange);
  const [selectedEffort, setEffort] = useChosen(effort, onChange);

  const allModels = choices(model);
  const searchable = allModels.length >= 6;
  const models = allModels.filter(
    (choice) =>
      !searchable ||
      `${choice.name} ${choice._meta?.["ox/provider"] ?? ""}`
        .toLowerCase()
        .includes(query.trim().toLowerCase()),
  );
  const current = allModels.find((choice) => choice.value === selectedModel);
  const levels = effort ? choices(effort) : [];
  const level = levels.findIndex((choice) => choice.value === selectedEffort);

  const choose = (choice: SelectOption) => setModel(choice.value);

  const navigate = (event: KeyboardEvent<HTMLElement>) => {
    let next = active;
    if (event.key === "ArrowDown") next = Math.min(active + 1, models.length - 1);
    else if (event.key === "ArrowUp") next = Math.max(active - 1, 0);
    else if (!searchable && event.key === "Home") next = 0;
    else if (!searchable && event.key === "End") next = models.length - 1;
    else if (event.key === "Enter" && models[active]) choose(models[active]);
    else return;
    event.preventDefault();
    setActive(next);
    if (!searchable) (list.current!.children[next] as HTMLButtonElement | undefined)?.focus();
  };

  return (
    <>
      <button
        type="button"
        className="select-picker model-picker"
        aria-haspopup="dialog"
        popoverTarget="model-menu"
        style={{ anchorName: "--model-picker" }}
      >
        <span>{current?.name ?? selectedModel}</span>
        {current?._meta?.["ox/provider"] && (
          <span className="provider-name">{current._meta["ox/provider"]}</span>
        )}
        {levels[level] && <span className="effort-name">{levels[level].name}</span>}
        <ChevronDown />
      </button>
      <div
        id="model-menu"
        className="menu model-menu"
        role="dialog"
        aria-label={model.name}
        popover="auto"
        style={{ positionAnchor: "--model-picker" }}
        onToggle={(event) => {
          if (event.newState !== "open") return;
          setQuery("");
          if (searchable) {
            setActive(0);
            search.current!.focus();
          } else {
            const index = Math.max(
              0,
              allModels.findIndex((choice) => choice.value === selectedModel),
            );
            setActive(index);
            (list.current!.children[index] as HTMLButtonElement | undefined)?.focus();
          }
        }}
      >
        {searchable && (
          <label className="search">
            <Search />
            <input
              ref={search}
              type="text"
              role="combobox"
              aria-label="Search models"
              aria-controls="model-list"
              aria-expanded="true"
              aria-activedescendant={models[active] && `model-${models[active].value}`}
              placeholder="Search models"
              value={query}
              onChange={(event) => {
                setQuery(event.target.value);
                setActive(0);
              }}
              onKeyDown={navigate}
            />
          </label>
        )}
        <div
          ref={list}
          id="model-list"
          className="menu-list"
          role="listbox"
          aria-label={model.name}
          onKeyDown={searchable ? undefined : navigate}
        >
          {models.map((choice, index) => (
            <button
              key={choice.value}
              id={`model-${choice.value}`}
              type="button"
              role="option"
              tabIndex={!searchable && index === active ? 0 : -1}
              aria-selected={choice.value === selectedModel}
              className={index === active ? "active" : undefined}
              onMouseEnter={(event) => {
                setActive(index);
                if (!searchable) event.currentTarget.focus();
              }}
              onFocus={() => setActive(index)}
              onClick={() => choose(choice)}
            >
              <span title={choice.name}>{choice.name}</span>
              {choice._meta?.["ox/provider"] && (
                <span className="provider-name">{choice._meta["ox/provider"]}</span>
              )}
            </button>
          ))}
        </div>
        {effort && (
          <div className="effort">
            <label htmlFor="effort-level">{effort.name}</label>
            <input
              id="effort-level"
              type="range"
              min={0}
              max={levels.length - 1}
              step={1}
              value={level}
              aria-valuetext={levels[level]?.name}
              style={{ "--position": position(level, levels.length) } as CSSProperties}
              onChange={(event) => setEffort(levels[Number(event.target.value)]!.value)}
            />
            <div className="stops" aria-hidden="true">
              {levels.map((choice, index) => (
                <span
                  key={choice.value}
                  style={{ "--position": position(index, levels.length) } as CSSProperties}
                >
                  {choice.name}
                </span>
              ))}
            </div>
          </div>
        )}
      </div>
    </>
  );
}

/**
 * The value of `option` to show, and a function that chooses one: the value
 * chosen last until the agent reports it, or until it refuses it, and
 * otherwise the agent's current value.
 */
function useChosen(
  option: SelectConfig | undefined,
  onChange: (option: SelectConfig, value: string) => Promise<boolean>,
) {
  const [chosen, setChosen] = useState<string | null>(null);
  const current = option?.currentValue;
  useEffect(() => {
    if (chosen === current) setChosen(null);
  }, [chosen, current]);

  const choose = async (value: string) => {
    if (!option || value === (chosen ?? current)) return;
    setChosen(value);
    const accepted = await onChange(option, value);
    if (!accepted) setChosen((latest) => (latest === value ? null : latest));
  };
  return [chosen ?? current, choose] as const;
}

/** Where stop `index` of `count` sits along a slider, from 0 to 1. */
function position(index: number, count: number): number {
  return count > 1 ? index / (count - 1) : 0;
}
