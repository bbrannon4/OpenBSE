import { useState, useRef, useEffect } from "react";
import type { ClassInfo, FieldInfo, ResolvedSchema } from "../lib/schema";
import {
  getCrossRefOptions,
  getArrayCrossRefOptions,
  isCrossRef,
  isArrayCrossRef,
} from "../lib/crossref";
import {
  validateField,
  validateNameUniqueness,
  extractUnit,
} from "../lib/validation";
import type { ValidationError } from "../lib/validation";

// eslint-disable-next-line @typescript-eslint/no-explicit-any
type Obj = Record<string, any>;
// eslint-disable-next-line @typescript-eslint/no-explicit-any
type Model = Record<string, any>;

interface ObjectEditorProps {
  classInfo: ClassInfo;
  instances: Obj[];
  model: Model;
  onUpdate: (index: number, updated: Obj) => void;
  onAdd: () => void;
  onDuplicate: (index: number) => void;
  onDelete: (index: number) => void;
  onMove: (index: number, direction: "up" | "down") => void;
}

/**
 * Sort fields: name first, then type, then required fields, then optional.
 */
function sortFields(fields: FieldInfo[]): FieldInfo[] {
  return [...fields].sort((a, b) => {
    if (a.name === "name") return -1;
    if (b.name === "name") return 1;
    if (a.name === "type") return -1;
    if (b.name === "type") return 1;
    if (a.required && !b.required) return -1;
    if (!a.required && b.required) return 1;
    return 0;
  });
}

export function ObjectEditor({
  classInfo,
  instances,
  model,
  onUpdate,
  onAdd,
  onDuplicate,
  onDelete,
  onMove,
}: ObjectEditorProps) {
  const [selectedIndex, setSelectedIndex] = useState(0);

  const idx = Math.min(selectedIndex, Math.max(0, instances.length - 1));
  const currentObj = instances[idx];
  const schema = classInfo.itemSchema;

  // Handle top-level oneOf classes (like controls)
  const isTopLevelOneOf =
    schema.type === "oneOf" && schema.oneOf && !schema.properties;

  // Handle singleton scalar properties (boolean, string, number with no sub-properties)
  const isScalarSingleton =
    !classInfo.isArray &&
    !schema.properties &&
    !isTopLevelOneOf &&
    (schema.type === "boolean" || schema.type === "string" || schema.type === "number" || schema.type === "integer");

  const fields = schema.properties
    ? sortFields(Object.values(schema.properties))
    : [];

  return (
    <div className="object-editor">
      <div className="object-editor-header">
        <div className="header-top-row">
          <h2>{classInfo.displayName}</h2>
          {classInfo.description && (
            <p className="class-description">{classInfo.description}</p>
          )}
        </div>

        {classInfo.isArray && (
          <div className="instance-bar">
            <div className="instance-selector">
              <label>Instance:</label>
              <select
                value={idx}
                onChange={(e) => setSelectedIndex(Number(e.target.value))}
                disabled={instances.length === 0}
              >
                {instances.map((inst, i) => (
                  <option key={i} value={i}>
                    {i + 1}. {getInstanceLabel(inst, classInfo)}
                  </option>
                ))}
                {instances.length === 0 && (
                  <option value={0}>(none)</option>
                )}
              </select>
              <span className="instance-nav">
                <button
                  title="Previous"
                  disabled={idx <= 0}
                  onClick={() => setSelectedIndex(idx - 1)}
                >
                  &lsaquo;
                </button>
                <span className="instance-position">
                  {instances.length > 0
                    ? `${idx + 1}/${instances.length}`
                    : "0/0"}
                </span>
                <button
                  title="Next"
                  disabled={idx >= instances.length - 1}
                  onClick={() => setSelectedIndex(idx + 1)}
                >
                  &rsaquo;
                </button>
              </span>
            </div>
            <div className="instance-actions">
              <button className="btn-add" onClick={onAdd} title="Add new">
                + New
              </button>
              <button
                className="btn-secondary"
                onClick={() => onDuplicate(idx)}
                disabled={instances.length === 0}
                title="Duplicate"
              >
                Dup
              </button>
              <button
                className="btn-secondary"
                onClick={() => onMove(idx, "up")}
                disabled={idx <= 0}
                title="Move up"
              >
                Up
              </button>
              <button
                className="btn-secondary"
                onClick={() => onMove(idx, "down")}
                disabled={idx >= instances.length - 1}
                title="Move down"
              >
                Dn
              </button>
              <button
                className="btn-danger"
                onClick={() => {
                  onDelete(idx);
                  setSelectedIndex(Math.max(0, idx - 1));
                }}
                disabled={instances.length === 0}
                title="Delete"
              >
                Del
              </button>
            </div>
          </div>
        )}
      </div>

      {instances.length === 0 && classInfo.isArray ? (
        <div className="empty-state">
          <p>No {classInfo.displayName.toLowerCase()} defined.</p>
          <button className="btn-add" onClick={onAdd}>
            + Add {classInfo.displayName.replace(/s$/, "")}
          </button>
        </div>
      ) : isScalarSingleton ? (
        <div className="fields-form-container">
          <div className="scalar-singleton">
            <ScalarSingletonEditor
              schema={schema}
              value={currentObj}
              onChange={(val) => onUpdate(idx, val as Obj)}
            />
          </div>
        </div>
      ) : isTopLevelOneOf ? (
        <div className="fields-form-container">
          <TopLevelOneOfEditor
            schema={schema}
            value={currentObj}
            model={model}
            onChange={(val) => onUpdate(idx, val)}
          />
        </div>
      ) : (
        <div className="fields-form-container">
          <table className="fields-table">
            <thead>
              <tr>
                <th className="col-name">Field</th>
                <th className="col-value">Value</th>
                <th className="col-description">Description</th>
              </tr>
            </thead>
            <tbody>
              {fields.map((field) => {
                let nameErrors: ValidationError[] | undefined;
                if (field.name === "name" && classInfo.isArray) {
                  const err = validateNameUniqueness(
                    (currentObj?.[field.name] as string) || "",
                    instances,
                    idx,
                  );
                  if (err) nameErrors = [err];
                }
                return (
                  <FieldEditor
                    key={field.name}
                    field={field}
                    value={currentObj?.[field.name]}
                    model={model}
                    onChange={(val) => {
                      if (!currentObj) return;
                      const updated = { ...currentObj };
                      if (val === undefined) {
                        delete updated[field.name];
                      } else {
                        updated[field.name] = val;
                      }
                      onUpdate(idx, updated);
                    }}
                    nameErrors={nameErrors}
                  />
                );
              })}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}

// ===== Scalar singleton editor (for summary_report, etc.) =====

function ScalarSingletonEditor({
  schema,
  value,
  onChange,
}: {
  schema: ResolvedSchema;
  value: unknown;
  onChange: (val: unknown) => void;
}) {
  if (schema.type === "boolean") {
    const checked = value === true || value === "true";
    return (
      <table className="fields-table">
        <thead>
          <tr>
            <th className="col-name">Value</th>
            <th className="col-description">Description</th>
          </tr>
        </thead>
        <tbody>
          <tr>
            <td className="col-value">
              <label className="checkbox-label">
                <input
                  type="checkbox"
                  checked={checked}
                  onChange={(e) => onChange(e.target.checked)}
                />
                <span>{checked ? "true" : "false"}</span>
              </label>
            </td>
            <td className="col-description">
              <span className="field-desc-text">
                {schema.description || ""}
                {schema.default !== undefined && (
                  <span className="default-hint"> (default: {String(schema.default)})</span>
                )}
              </span>
            </td>
          </tr>
        </tbody>
      </table>
    );
  }

  if (schema.type === "number" || schema.type === "integer") {
    return (
      <table className="fields-table">
        <thead>
          <tr>
            <th className="col-name">Value</th>
            <th className="col-description">Description</th>
          </tr>
        </thead>
        <tbody>
          <tr>
            <td className="col-value">
              <input
                type="number"
                className="field-number"
                value={value != null ? String(value) : ""}
                placeholder={schema.default != null ? String(schema.default) : ""}
                step={schema.type === "integer" ? 1 : "any"}
                onChange={(e) => {
                  const v = e.target.value;
                  onChange(v === "" ? undefined : schema.type === "integer" ? parseInt(v, 10) : parseFloat(v));
                }}
              />
            </td>
            <td className="col-description">
              <span className="field-desc-text">{schema.description || ""}</span>
            </td>
          </tr>
        </tbody>
      </table>
    );
  }

  // String fallback
  return (
    <table className="fields-table">
      <thead>
        <tr>
          <th className="col-name">Value</th>
          <th className="col-description">Description</th>
        </tr>
      </thead>
      <tbody>
        <tr>
          <td className="col-value">
            <input
              type="text"
              className="field-text"
              value={value != null ? String(value) : ""}
              placeholder={schema.default != null ? String(schema.default) : ""}
              onChange={(e) => onChange(e.target.value || undefined)}
            />
          </td>
          <td className="col-description">
            <span className="field-desc-text">{schema.description || ""}</span>
          </td>
        </tr>
      </tbody>
    </table>
  );
}

function getInstanceLabel(obj: Obj | undefined, classInfo: ClassInfo): string {
  if (!obj) return "(empty)";
  if (obj.name) return String(obj.name);
  if (obj.file) return String(obj.file);
  if (obj.zone && obj.type) return `${obj.type} in ${obj.zone}`;
  const schema = classInfo.itemSchema;
  if (schema.properties) {
    for (const f of Object.values(schema.properties)) {
      if (f.type === "string" && obj[f.name] && f.name !== "type") {
        return String(obj[f.name]);
      }
    }
  }
  return "(unnamed)";
}

// ===== Top-level oneOf editor (for controls) =====

function TopLevelOneOfEditor({
  schema,
  value,
  model,
  onChange,
}: {
  schema: ResolvedSchema;
  value: Obj;
  model: Model;
  onChange: (val: Obj) => void;
}) {
  const obj = value || {};
  const currentType = obj.type || "";

  const variants = new Map<string, ResolvedSchema>();
  for (const v of schema.oneOf || []) {
    const typeField = v.properties?.["type"];
    if (typeField?.constValue !== undefined) {
      variants.set(String(typeField.constValue), v);
    }
  }

  const variantSchema = variants.get(currentType);
  const variantFields = variantSchema?.properties
    ? sortFields(
        Object.values(variantSchema.properties).filter(
          (f) => f.name !== "type"
        )
      )
    : [];

  return (
    <table className="fields-table">
      <thead>
        <tr>
          <th className="col-name">Field</th>
          <th className="col-value">Value</th>
          <th className="col-description">Description</th>
        </tr>
      </thead>
      <tbody>
        <tr className="field-required">
          <td className="col-name">
            <code>type</code>
            <span className="required-marker">*</span>
          </td>
          <td className="col-value">
            <select
              className="field-select"
              value={currentType}
              onChange={(e) => {
                const newType = e.target.value;
                const newObj: Obj = { type: newType };
                if (obj.name) newObj.name = obj.name;
                onChange(newObj);
              }}
            >
              <option value="">-- select type --</option>
              {[...variants.keys()].map((t) => (
                <option key={t} value={t}>
                  {t}
                </option>
              ))}
            </select>
          </td>
          <td className="col-description">
            <span className="field-desc-text">
              {schema.description || "Type discriminator."}
            </span>
          </td>
        </tr>
        {variantFields.map((f) => (
          <FieldEditor
            key={f.name}
            field={f}
            value={obj[f.name]}
            model={model}
            onChange={(val) => {
              const updated = { ...obj };
              if (val === undefined) {
                delete updated[f.name];
              } else {
                updated[f.name] = val;
              }
              onChange(updated);
            }}
          />
        ))}
      </tbody>
    </table>
  );
}

// ===== Field Editor =====

interface FieldEditorProps {
  field: FieldInfo;
  value: unknown;
  model: Model;
  onChange: (val: unknown) => void;
}

function FieldEditor({ field, value, model, onChange, nameErrors }: FieldEditorProps & { nameErrors?: ValidationError[] }) {
  const errors = [
    ...validateField(field, value),
    ...(nameErrors || []),
  ];

  return (
    <tr className={`${field.required ? "field-required" : "field-optional"} ${errors.length > 0 ? "field-has-errors" : ""}`}>
      <td className="col-name">
        <code>{field.name}</code>
        {field.required && <span className="required-marker">*</span>}
      </td>
      <td className="col-value">
        <FieldInput field={field} value={value} model={model} onChange={onChange} />
        {errors.length > 0 && <ValidationMessages errors={errors} />}
      </td>
      <td className="col-description">
        <span className="field-desc-text">{field.description || ""}</span>
      </td>
    </tr>
  );
}

function ValidationMessages({ errors }: { errors: ValidationError[] }) {
  return (
    <div className="validation-messages">
      {errors.map((err, i) => (
        <span key={i} className={`validation-msg validation-${err.severity}`}>
          {err.message}
        </span>
      ))}
    </div>
  );
}

function FieldInput({ field, value, model, onChange }: FieldEditorProps) {
  // const field
  if (field.constValue !== undefined) {
    return <span className="const-value">{String(field.constValue)}</span>;
  }

  // AutosizeOrNumber
  if (field.type === "oneOf" && field.oneOf) {
    const hasAutosize = field.oneOf.some((v) => v.constValue === "autosize");
    const hasNumber = field.oneOf.some(
      (v) => v.type === "number" || v.type === "integer"
    );
    if (hasAutosize && hasNumber) {
      return (
        <AutosizeOrNumberInput
          value={value}
          onChange={onChange}
          field={field}
          model={model}
        />
      );
    }

    // BoundaryCondition
    const hasStringEnum = field.oneOf.some((v) => v.enumValues);
    const hasObjectZone = field.oneOf.some(
      (v) => v.type === "object" && v.properties?.["zone"]
    );
    if (hasStringEnum && hasObjectZone) {
      return (
        <BoundaryConditionInput
          value={value}
          onChange={onChange}
          field={field}
          model={model}
        />
      );
    }

    // Discriminated union
    const hasTypeDiscriminator = field.oneOf.some(
      (v) => v.properties?.["type"]?.constValue !== undefined
    );
    if (hasTypeDiscriminator) {
      return (
        <DiscriminatedUnionInput
          value={value}
          onChange={onChange}
          field={field}
          model={model}
        />
      );
    }
  }

  // Enum dropdown
  if (field.enumValues) {
    return (
      <select
        className="field-select"
        value={value != null ? String(value) : ""}
        onChange={(e) => onChange(e.target.value || undefined)}
      >
        {!field.required && <option value="">--</option>}
        {field.enumValues.map((v) => (
          <option key={v} value={v}>
            {v}
          </option>
        ))}
      </select>
    );
  }

  // Boolean
  if (field.type === "boolean") {
    const checked = value === true || value === "true";
    return (
      <label className="checkbox-label">
        <input
          type="checkbox"
          checked={checked}
          onChange={(e) => onChange(e.target.checked)}
        />
        <span>{checked ? "true" : "false"}</span>
      </label>
    );
  }

  // Number / Integer
  if (field.type === "number" || field.type === "integer") {
    const unit = extractUnit(field.description);
    return (
      <div className="number-with-unit">
        <input
          type="number"
          className="field-number"
          value={value != null ? String(value) : ""}
          placeholder={field.default != null ? String(field.default) : ""}
          min={field.minimum}
          max={field.maximum}
          step={field.type === "integer" ? 1 : "any"}
          onChange={(e) => {
            const v = e.target.value;
            if (v === "") {
              onChange(undefined);
            } else {
              onChange(
                field.type === "integer" ? parseInt(v, 10) : parseFloat(v)
              );
            }
          }}
        />
        {unit && <span className="field-unit">{unit}</span>}
      </div>
    );
  }

  // Array of strings
  if (field.type === "array" && field.items && field.items.type === "string") {
    if (isArrayCrossRef(field.name)) {
      return (
        <CrossRefArrayInput
          value={value as string[] | undefined}
          onChange={onChange}
          options={getArrayCrossRefOptions(field.name, model)}
        />
      );
    }
    return (
      <SimpleArrayInput
        value={value as unknown[] | undefined}
        onChange={onChange}
        itemType="string"
      />
    );
  }

  // Array of numbers
  if (field.type === "array" && field.items && field.items.type === "number") {
    return (
      <SimpleArrayInput
        value={value as unknown[] | undefined}
        onChange={onChange}
        itemType="number"
      />
    );
  }

  // Array of objects
  if (field.type === "array" && field.items) {
    const arr = Array.isArray(value) ? value : [];

    if (
      (field.items.type === "oneOf" && field.items.oneOf) ||
      (field.items.type === "object" && field.items.properties)
    ) {
      return (
        <NestedArrayEditor
          items={arr}
          itemSchema={field.items}
          model={model}
          onChange={onChange}
        />
      );
    }

    return <span className="nested-indicator">[{arr.length} items]</span>;
  }

  // Nested object
  if (field.type === "object" && field.properties) {
    return (
      <NestedObjectInput
        properties={field.properties}
        value={(value as Obj) || {}}
        model={model}
        onChange={onChange}
      />
    );
  }

  // String — cross-reference
  if (field.type === "string" && isCrossRef(field.name)) {
    return (
      <ComboboxInput
        value={value != null ? String(value) : ""}
        onChange={(v) => onChange(v || undefined)}
        options={getCrossRefOptions(field.name, model)}
        placeholder={field.default != null ? String(field.default) : ""}
      />
    );
  }

  // String (default)
  return (
    <input
      type="text"
      className="field-text"
      value={value != null ? String(value) : ""}
      placeholder={field.default != null ? String(field.default) : ""}
      onChange={(e) => onChange(e.target.value || undefined)}
    />
  );
}

// ===== Combobox =====

function ComboboxInput({
  value,
  onChange,
  options,
  placeholder,
}: {
  value: string;
  onChange: (val: string) => void;
  options: string[];
  placeholder?: string;
}) {
  const [isOpen, setIsOpen] = useState(false);
  const [filter, setFilter] = useState("");
  const wrapperRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    function handleClickOutside(e: MouseEvent) {
      if (
        wrapperRef.current &&
        !wrapperRef.current.contains(e.target as Node)
      ) {
        setIsOpen(false);
      }
    }
    document.addEventListener("mousedown", handleClickOutside);
    return () => document.removeEventListener("mousedown", handleClickOutside);
  }, []);

  const filtered = filter
    ? options.filter((o) => o.toLowerCase().includes(filter.toLowerCase()))
    : options;

  const isInvalid = value !== "" && options.length > 0 && !options.includes(value);

  return (
    <div className="combobox" ref={wrapperRef}>
      <input
        type="text"
        className={`field-text ${isInvalid ? "field-invalid" : ""}`}
        value={value}
        placeholder={placeholder}
        onChange={(e) => {
          onChange(e.target.value);
          setFilter(e.target.value);
          setIsOpen(true);
        }}
        onFocus={() => {
          setFilter("");
          setIsOpen(true);
        }}
      />
      {isOpen && filtered.length > 0 && (
        <div className="combobox-dropdown">
          {filtered.map((opt) => (
            <button
              key={opt}
              className={`combobox-option ${opt === value ? "selected" : ""}`}
              onMouseDown={(e) => {
                e.preventDefault();
                onChange(opt);
                setIsOpen(false);
                setFilter("");
              }}
            >
              {opt}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

// ===== Cross-ref array (zones) =====

function CrossRefArrayInput({
  value,
  onChange,
  options,
}: {
  value: string[] | undefined;
  onChange: (val: unknown) => void;
  options: string[];
}) {
  const arr = Array.isArray(value) ? value : [];

  return (
    <div className="simple-array">
      {arr.map((item, i) => (
        <div key={i} className="array-item">
          <ComboboxInput
            value={item}
            onChange={(v) => {
              const newArr = [...arr];
              newArr[i] = v;
              onChange(newArr);
            }}
            options={options}
          />
          <button
            className="btn-icon"
            onClick={() => {
              const newArr = arr.filter((_, j) => j !== i);
              onChange(newArr.length > 0 ? newArr : undefined);
            }}
          >
            x
          </button>
        </div>
      ))}
      <button
        className="btn-secondary btn-small"
        onClick={() => onChange([...arr, ""])}
      >
        + Add
      </button>
    </div>
  );
}

// ===== Autosize or Number =====

function AutosizeOrNumberInput({ value, onChange, field }: FieldEditorProps) {
  const isAutosize = value === "autosize";
  const numericField = field.oneOf?.find(
    (v) => v.type === "number" || v.type === "integer"
  );
  const unit = extractUnit(field.description);

  return (
    <div className="autosize-input">
      <label className="autosize-toggle">
        <input
          type="checkbox"
          checked={isAutosize}
          onChange={(e) => onChange(e.target.checked ? "autosize" : undefined)}
        />
        <span>autosize</span>
      </label>
      {!isAutosize && (
        <div className="number-with-unit">
          <input
            type="number"
            className="field-number"
            value={value != null && value !== "autosize" ? String(value) : ""}
            placeholder={field.default != null ? String(field.default) : ""}
            min={numericField?.minimum}
            max={numericField?.maximum}
            step="any"
            onChange={(e) => {
              const v = e.target.value;
              onChange(v === "" ? undefined : parseFloat(v));
            }}
          />
          {unit && <span className="field-unit">{unit}</span>}
        </div>
      )}
    </div>
  );
}

// ===== Boundary Condition =====

function BoundaryConditionInput({ value, onChange, field, model }: FieldEditorProps) {
  const enumVariant = field.oneOf?.find((v) => v.enumValues);
  const enumValues = enumVariant?.enumValues || [];
  const isZoneRef =
    typeof value === "object" && value !== null && "zone" in value;
  const currentMode = isZoneRef ? "__zone__" : (value as string) || "";
  const zoneOptions = getCrossRefOptions("zone", model);

  return (
    <div className="boundary-input">
      <select
        className="field-select"
        value={currentMode}
        onChange={(e) => {
          const v = e.target.value;
          if (v === "__zone__") {
            onChange({ zone: "" });
          } else if (v === "") {
            onChange(undefined);
          } else {
            onChange(v);
          }
        }}
      >
        <option value="">--</option>
        {enumValues.map((v) => (
          <option key={v} value={v}>
            {v}
          </option>
        ))}
        <option value="__zone__">zone reference...</option>
      </select>
      {isZoneRef && (
        <ComboboxInput
          value={(value as Obj).zone || ""}
          onChange={(v) => onChange({ zone: v })}
          options={zoneOptions}
          placeholder="Zone name"
        />
      )}
    </div>
  );
}

// ===== Discriminated Union =====

function DiscriminatedUnionInput({ value, onChange, field, model }: FieldEditorProps) {
  const obj = (value as Obj) || {};
  const currentType = obj.type || "";

  const variants = new Map<string, ResolvedSchema>();
  for (const v of field.oneOf || []) {
    const typeField = v.properties?.["type"];
    if (typeField?.constValue !== undefined) {
      variants.set(String(typeField.constValue), v);
    }
  }

  const variantSchema = variants.get(currentType);
  const variantFields = variantSchema?.properties
    ? sortFields(
        Object.values(variantSchema.properties).filter(
          (f) => f.name !== "type"
        )
      )
    : [];

  return (
    <div className="discriminated-union">
      <div className="union-type-selector">
        <label>type:</label>
        <select
          className="field-select"
          value={currentType}
          onChange={(e) => {
            const newType = e.target.value;
            const newObj: Obj = { type: newType };
            if (obj.name) newObj.name = obj.name;
            onChange(newObj);
          }}
        >
          <option value="">-- select type --</option>
          {[...variants.keys()].map((t) => (
            <option key={t} value={t}>
              {t}
            </option>
          ))}
        </select>
      </div>
      {variantFields.length > 0 && (
        <table className="nested-fields-table">
          <tbody>
            {variantFields.map((f) => (
              <FieldEditor
                key={f.name}
                field={f}
                value={obj[f.name]}
                model={model}
                onChange={(val) => {
                  const updated = { ...obj };
                  if (val === undefined) {
                    delete updated[f.name];
                  } else {
                    updated[f.name] = val;
                  }
                  onChange(updated);
                }}
              />
            ))}
          </tbody>
        </table>
      )}
    </div>
  );
}

// ===== Nested Object =====

function NestedObjectInput({
  properties,
  value,
  model,
  onChange,
}: {
  properties: Record<string, FieldInfo>;
  value: Obj;
  model: Model;
  onChange: (val: unknown) => void;
}) {
  const fields = sortFields(Object.values(properties));
  return (
    <div className="nested-object">
      <table className="nested-fields-table">
        <tbody>
          {fields.map((f) => (
            <FieldEditor
              key={f.name}
              field={f}
              value={value[f.name]}
              model={model}
              onChange={(val) => {
                const updated = { ...value };
                if (val === undefined) {
                  delete updated[f.name];
                } else {
                  updated[f.name] = val;
                }
                if (Object.keys(updated).length === 0) {
                  onChange(undefined);
                } else {
                  onChange(updated);
                }
              }}
            />
          ))}
        </tbody>
      </table>
    </div>
  );
}

// ===== Nested Array Editor =====

function NestedArrayEditor({
  items,
  itemSchema,
  model,
  onChange,
}: {
  items: Obj[];
  itemSchema: ResolvedSchema;
  model: Model;
  onChange: (val: unknown) => void;
}) {
  const [expandedIdx, setExpandedIdx] = useState<number | null>(
    items.length > 0 ? 0 : null
  );

  const isDiscriminated =
    itemSchema.type === "oneOf" && itemSchema.oneOf !== undefined;

  function getItemLabel(item: Obj, i: number): string {
    const parts: string[] = [];
    if (item.type) parts.push(item.type);
    if (item.name) parts.push(item.name);
    if (item.zone) parts.push(item.zone);
    return parts.length > 0 ? parts.join(": ") : `Item ${i + 1}`;
  }

  return (
    <div className="nested-array">
      <div className="nested-array-header">
        <span className="nested-array-count">{items.length} items</span>
        <button
          className="btn-secondary btn-small"
          onClick={() => {
            const newItem: Obj = {};
            const newItems = [...items, newItem];
            onChange(newItems);
            setExpandedIdx(newItems.length - 1);
          }}
        >
          + Add
        </button>
      </div>
      {items.map((item, i) => (
        <div key={i} className="nested-array-item">
          <div
            className={`nested-array-item-header ${expandedIdx === i ? "expanded" : ""}`}
            onClick={() => setExpandedIdx(expandedIdx === i ? null : i)}
          >
            <span className="expand-arrow">
              {expandedIdx === i ? "\u25BC" : "\u25B6"}
            </span>
            <span className="nested-item-label">{getItemLabel(item, i)}</span>
            <span className="nested-item-actions">
              <button
                className="btn-icon"
                onClick={(e) => {
                  e.stopPropagation();
                  if (i > 0) {
                    const newItems = [...items];
                    [newItems[i - 1], newItems[i]] = [newItems[i], newItems[i - 1]];
                    onChange(newItems);
                    setExpandedIdx(i - 1);
                  }
                }}
                title="Move up"
                style={{ opacity: i > 0 ? 1 : 0.3 }}
              >
                &#9650;
              </button>
              <button
                className="btn-icon"
                onClick={(e) => {
                  e.stopPropagation();
                  if (i < items.length - 1) {
                    const newItems = [...items];
                    [newItems[i], newItems[i + 1]] = [newItems[i + 1], newItems[i]];
                    onChange(newItems);
                    setExpandedIdx(i + 1);
                  }
                }}
                title="Move down"
                style={{ opacity: i < items.length - 1 ? 1 : 0.3 }}
              >
                &#9660;
              </button>
              <button
                className="btn-icon"
                onClick={(e) => {
                  e.stopPropagation();
                  const newItems = items.filter((_, j) => j !== i);
                  onChange(newItems.length > 0 ? newItems : []);
                  if (expandedIdx === i) setExpandedIdx(null);
                }}
                title="Remove"
              >
                x
              </button>
            </span>
          </div>
          {expandedIdx === i && (
            <div className="nested-array-item-body">
              {isDiscriminated ? (
                <DiscriminatedUnionInput
                  field={{
                    name: `item_${i}`,
                    type: "oneOf",
                    required: true,
                    oneOf: itemSchema.oneOf,
                  }}
                  value={item}
                  model={model}
                  onChange={(val) => {
                    const newItems = [...items];
                    newItems[i] = val as Obj;
                    onChange(newItems);
                  }}
                />
              ) : itemSchema.properties ? (
                <NestedObjectInput
                  properties={itemSchema.properties}
                  value={item}
                  model={model}
                  onChange={(val) => {
                    const newItems = [...items];
                    newItems[i] = (val as Obj) || {};
                    onChange(newItems);
                  }}
                />
              ) : null}
            </div>
          )}
        </div>
      ))}
    </div>
  );
}

// ===== Simple Array =====

function SimpleArrayInput({
  value,
  onChange,
  itemType,
}: {
  value: unknown[] | undefined;
  onChange: (val: unknown) => void;
  itemType: string;
}) {
  const arr = Array.isArray(value) ? value : [];
  const isNumeric = itemType === "number";

  if (isNumeric) {
    return (
      <input
        type="text"
        className="field-text array-text"
        value={arr.join(", ")}
        placeholder="Comma-separated values, e.g. 1, 1, 0.5, 0, ..."
        onChange={(e) => {
          const parts = e.target.value
            .split(",")
            .map((s) => s.trim())
            .filter((s) => s !== "")
            .map(Number);
          onChange(parts.length > 0 ? parts : undefined);
        }}
      />
    );
  }

  return (
    <div className="simple-array">
      {arr.map((item, i) => (
        <div key={i} className="array-item">
          <input
            type="text"
            className="field-text"
            value={String(item)}
            onChange={(e) => {
              const newArr = [...arr];
              newArr[i] = isNumeric ? Number(e.target.value) : e.target.value;
              onChange(newArr);
            }}
          />
          <button
            className="btn-icon"
            onClick={() => {
              const newArr = arr.filter((_, j) => j !== i);
              onChange(newArr.length > 0 ? newArr : undefined);
            }}
          >
            x
          </button>
        </div>
      ))}
      <button
        className="btn-secondary btn-small"
        onClick={() => onChange([...arr, isNumeric ? 0 : ""])}
      >
        + Add
      </button>
    </div>
  );
}
