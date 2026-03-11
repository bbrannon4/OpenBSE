/**
 * Validation utilities for OpenBSE editor fields.
 * Pure functions that compute validation errors for field + value pairs.
 */

import type { FieldInfo } from "./schema";

export interface ValidationError {
  message: string;
  severity: "error" | "warning";
}

/**
 * Validate a single field's value against its schema constraints.
 * Returns an array of errors (empty if valid).
 */
export function validateField(
  field: FieldInfo,
  value: unknown,
): ValidationError[] {
  const errors: ValidationError[] = [];

  // Required field is empty
  if (field.required && (value === undefined || value === null || value === "")) {
    errors.push({
      message: "Required field",
      severity: "warning",
    });
  }

  // Number bounds
  if (
    (field.type === "number" || field.type === "integer") &&
    value !== undefined &&
    value !== null &&
    typeof value === "number"
  ) {
    if (field.minimum !== undefined && value < field.minimum) {
      errors.push({
        message: `Must be at least ${field.minimum}`,
        severity: "error",
      });
    }
    if (field.maximum !== undefined && value > field.maximum) {
      errors.push({
        message: `Must be at most ${field.maximum}`,
        severity: "error",
      });
    }
  }

  // Array length constraints (minItems / maxItems)
  if (field.type === "array" && Array.isArray(value)) {
    if (
      field.minItems !== undefined &&
      field.maxItems !== undefined &&
      field.minItems === field.maxItems &&
      value.length !== field.minItems
    ) {
      errors.push({
        message: `Exactly ${field.minItems} values required (has ${value.length})`,
        severity: "error",
      });
    } else {
      if (field.minItems !== undefined && value.length < field.minItems) {
        errors.push({
          message: `Needs at least ${field.minItems} items (has ${value.length})`,
          severity: "error",
        });
      }
      if (field.maxItems !== undefined && value.length > field.maxItems) {
        errors.push({
          message: `At most ${field.maxItems} items allowed (has ${value.length})`,
          severity: "error",
        });
      }
    }
  }

  return errors;
}

/**
 * Check if a name is unique within a collection of instances.
 * Returns a validation error if the name is duplicated.
 */
export function validateNameUniqueness(
  name: string,
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  allInstances: Record<string, any>[],
  currentIndex: number,
): ValidationError | null {
  if (!name) return null;
  const isDuplicate = allInstances.some(
    (inst, i) => i !== currentIndex && inst.name === name,
  );
  if (isDuplicate) {
    return {
      message: "Duplicate name — must be unique",
      severity: "error",
    };
  }
  return null;
}

/**
 * Extract a unit string from a field description.
 * Matches the bracketed suffix pattern used in OpenBSE schema descriptions,
 * e.g. "Capacity [W]" → "W", "Temperature [°C]" → "°C".
 * Returns null if no unit found or if it's a numeric range like [0-1].
 */
export function extractUnit(description: string | undefined): string | null {
  if (!description) return null;
  const match = description.match(/\[([^\]]+)\]$/);
  if (!match) return null;
  const unit = match[1];
  // Skip pure numeric ranges like "0-1"
  if (/^\d+(\.\d+)?-\d+(\.\d+)?$/.test(unit)) return null;
  return unit;
}
