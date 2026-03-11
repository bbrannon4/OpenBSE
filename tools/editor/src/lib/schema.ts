/**
 * JSON Schema parser for OpenBSE schema.
 * Resolves $ref/$defs and extracts class metadata for the editor UI.
 */

export interface FieldInfo {
  name: string;
  type: FieldType;
  description?: string;
  required: boolean;
  default?: unknown;
  minimum?: number;
  maximum?: number;
  enumValues?: string[];
  constValue?: unknown;
  items?: ResolvedSchema;
  minItems?: number;
  maxItems?: number;
  properties?: Record<string, FieldInfo>;
  /** For oneOf fields (like AutosizeOrNumber, Equipment) */
  oneOf?: ResolvedSchema[];
  /** For $ref fields, the definition name */
  refName?: string;
}

export type FieldType =
  | "string"
  | "number"
  | "integer"
  | "boolean"
  | "array"
  | "object"
  | "oneOf"
  | "unknown";

export interface ResolvedSchema {
  type: FieldType;
  description?: string;
  default?: unknown;
  minimum?: number;
  maximum?: number;
  enumValues?: string[];
  constValue?: unknown;
  items?: ResolvedSchema;
  minItems?: number;
  maxItems?: number;
  properties?: Record<string, FieldInfo>;
  required?: string[];
  oneOf?: ResolvedSchema[];
  refName?: string;
}

export interface ClassInfo {
  /** The top-level property key, e.g. "air_loops" */
  key: string;
  /** Display name, e.g. "Air Loops" */
  displayName: string;
  /** Description from the schema */
  description?: string;
  /** Whether this is an array of objects vs a scalar/object property */
  isArray: boolean;
  /** The resolved item schema (for arrays) or the property schema itself */
  itemSchema: ResolvedSchema;
}

/** Parse a schema key into a display name: "air_loops" -> "Air Loops" */
function toDisplayName(key: string): string {
  return key
    .split("_")
    .map((w) => w.charAt(0).toUpperCase() + w.slice(1))
    .join(" ");
}

/**
 * Resolve a JSON Schema node, following $ref pointers into $defs.
 */
function resolveSchema(
  node: Record<string, unknown>,
  defs: Record<string, Record<string, unknown>>
): ResolvedSchema {
  // Follow $ref
  if (node["$ref"]) {
    const ref = node["$ref"] as string;
    const defName = ref.replace("#/$defs/", "");
    const def = defs[defName];
    if (!def) {
      return { type: "unknown", description: `Unresolved ref: ${ref}` };
    }
    const resolved = resolveSchema(def, defs);
    // Merge description from the referencing node if present
    if (node["description"] && !resolved.description) {
      resolved.description = node["description"] as string;
    }
    // Overlay description from the reference site (closer context wins)
    if (node["description"]) {
      resolved.description = node["description"] as string;
    }
    resolved.refName = defName;
    // Carry over default from reference site
    if (node["default"] !== undefined && resolved.default === undefined) {
      resolved.default = node["default"];
    }
    return resolved;
  }

  // oneOf
  if (node["oneOf"]) {
    const variants = (node["oneOf"] as Record<string, unknown>[]).map((v) =>
      resolveSchema(v, defs)
    );
    return {
      type: "oneOf",
      description: node["description"] as string | undefined,
      oneOf: variants,
      default: node["default"] as unknown,
    };
  }

  const type = (node["type"] as string) || "unknown";

  const result: ResolvedSchema = {
    type: type as FieldType,
    description: node["description"] as string | undefined,
    default: node["default"] as unknown,
    minimum: node["minimum"] as number | undefined,
    maximum: node["maximum"] as number | undefined,
  };

  // enum
  if (node["enum"]) {
    result.enumValues = node["enum"] as string[];
  }

  // const
  if (node["const"] !== undefined) {
    result.constValue = node["const"];
  }

  // array items
  if (type === "array" && node["items"]) {
    result.items = resolveSchema(
      node["items"] as Record<string, unknown>,
      defs
    );
    if (node["minItems"] !== undefined) {
      result.minItems = node["minItems"] as number;
    }
    if (node["maxItems"] !== undefined) {
      result.maxItems = node["maxItems"] as number;
    }
  }

  // object properties
  if (type === "object" && node["properties"]) {
    const props = node["properties"] as Record<
      string,
      Record<string, unknown>
    >;
    const requiredFields = (node["required"] as string[]) || [];
    result.required = requiredFields;
    result.properties = {};
    for (const [propName, propSchema] of Object.entries(props)) {
      const resolved = resolveSchema(propSchema, defs);
      result.properties[propName] = {
        name: propName,
        type: resolved.type,
        description: resolved.description,
        required: requiredFields.includes(propName),
        default: propSchema["default"] !== undefined ? propSchema["default"] : resolved.default,
        minimum: resolved.minimum,
        maximum: resolved.maximum,
        enumValues: resolved.enumValues,
        constValue: resolved.constValue,
        items: resolved.items,
        minItems: resolved.minItems,
        maxItems: resolved.maxItems,
        properties: resolved.properties,
        oneOf: resolved.oneOf,
        refName: resolved.refName,
      };
    }
  }

  return result;
}

/**
 * Parse the raw OpenBSE JSON Schema and extract all top-level classes.
 */
export function parseSchema(
  rawSchema: Record<string, unknown>
): ClassInfo[] {
  const defs = (rawSchema["$defs"] || {}) as Record<
    string,
    Record<string, unknown>
  >;
  const properties = (rawSchema["properties"] || {}) as Record<
    string,
    Record<string, unknown>
  >;

  const classes: ClassInfo[] = [];

  for (const [key, propSchema] of Object.entries(properties)) {
    const resolved = resolveSchema(propSchema, defs);

    if (resolved.type === "array" && resolved.items) {
      classes.push({
        key,
        displayName: toDisplayName(key),
        description: resolved.description,
        isArray: true,
        itemSchema: resolved.items,
      });
    } else {
      classes.push({
        key,
        displayName: toDisplayName(key),
        description: resolved.description,
        isArray: false,
        itemSchema: resolved,
      });
    }
  }

  return classes;
}

/**
 * Count the number of fields in a resolved schema (for display).
 */
export function countFields(schema: ResolvedSchema): number {
  if (schema.properties) {
    return Object.keys(schema.properties).length;
  }
  return 0;
}

/**
 * Get the $defs names for reference (useful for listing defined types).
 */
export function getDefinitionNames(
  rawSchema: Record<string, unknown>
): string[] {
  const defs = rawSchema["$defs"] as Record<string, unknown> | undefined;
  return defs ? Object.keys(defs) : [];
}
