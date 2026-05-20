/**
 * Simple SVG path icons for HVAC components.
 * Each returns an SVG string for use in a 20x20 viewBox.
 * Designed to be immediately recognizable at small sizes.
 */

import type { HvacNodeType } from "./hvac-graph";

// All icons use a 20x20 viewBox, stroke-based for clarity
const ICONS: Record<HvacNodeType, string> = {
  // Fan: circle with curved blades (impeller symbol)
  fan: `<circle cx="10" cy="10" r="7" fill="none" stroke="currentColor" stroke-width="1.5"/>
    <path d="M10 3 C12 6, 14 8, 10 10 C6 8, 8 6, 10 3Z" fill="currentColor" opacity="0.7"/>
    <path d="M17 10 C14 12, 12 14, 10 10 C12 6, 14 8, 17 10Z" fill="currentColor" opacity="0.7"/>
    <path d="M10 17 C8 14, 6 12, 10 10 C14 12, 12 14, 10 17Z" fill="currentColor" opacity="0.7"/>
    <path d="M3 10 C6 8, 8 6, 10 10 C8 14, 6 12, 3 10Z" fill="currentColor" opacity="0.7"/>`,

  // Heating coil: wavy lines (heat exchanger symbol)
  heating_coil: `<path d="M4 6 Q7 4, 10 6 Q13 8, 16 6" fill="none" stroke="currentColor" stroke-width="1.5"/>
    <path d="M4 10 Q7 8, 10 10 Q13 12, 16 10" fill="none" stroke="currentColor" stroke-width="1.5"/>
    <path d="M4 14 Q7 12, 10 14 Q13 16, 16 14" fill="none" stroke="currentColor" stroke-width="1.5"/>`,

  // Cooling coil: wavy lines with snowflake-like marks
  cooling_coil: `<path d="M4 6 Q7 4, 10 6 Q13 8, 16 6" fill="none" stroke="currentColor" stroke-width="1.5"/>
    <path d="M4 10 Q7 8, 10 10 Q13 12, 16 10" fill="none" stroke="currentColor" stroke-width="1.5"/>
    <path d="M4 14 Q7 12, 10 14 Q13 16, 16 14" fill="none" stroke="currentColor" stroke-width="1.5"/>
    <circle cx="3" cy="10" r="1" fill="currentColor"/>
    <circle cx="17" cy="10" r="1" fill="currentColor"/>`,

  // Zone: simple room/box with roof
  zone: `<rect x="3" y="7" width="14" height="10" rx="1" fill="none" stroke="currentColor" stroke-width="1.5"/>
    <path d="M2 7 L10 2 L18 7" fill="none" stroke="currentColor" stroke-width="1.5"/>
    <rect x="8" y="12" width="4" height="5" fill="none" stroke="currentColor" stroke-width="1"/>`,

  // OA intake: arrow pointing right into a vent/grille
  oa_intake: `<path d="M2 10 L8 10" stroke="currentColor" stroke-width="1.5" marker-end="none"/>
    <polygon points="7,7 11,10 7,13" fill="currentColor"/>
    <line x1="13" y1="4" x2="13" y2="16" stroke="currentColor" stroke-width="1.5"/>
    <line x1="15" y1="4" x2="15" y2="16" stroke="currentColor" stroke-width="1.5"/>
    <line x1="17" y1="4" x2="17" y2="16" stroke="currentColor" stroke-width="1.5"/>`,

  // Terminal (VAV/PFP): damper symbol - rectangle with diagonal line
  terminal: `<rect x="3" y="4" width="14" height="12" rx="1" fill="none" stroke="currentColor" stroke-width="1.5"/>
    <line x1="3" y1="16" x2="17" y2="4" stroke="currentColor" stroke-width="1.5"/>
    <circle cx="10" cy="10" r="1.5" fill="currentColor"/>`,

  // Pump: circle with arrow (standard P&ID pump symbol)
  pump: `<circle cx="10" cy="10" r="7" fill="none" stroke="currentColor" stroke-width="1.5"/>
    <polygon points="10,3 17,13 3,13" fill="none" stroke="currentColor" stroke-width="1.5"/>`,

  // Boiler: flame inside a box
  boiler: `<rect x="3" y="3" width="14" height="14" rx="2" fill="none" stroke="currentColor" stroke-width="1.5"/>
    <path d="M10 14 C8 11, 6 9, 8 7 C9 6, 10 7, 10 8 C10 7, 11 6, 12 7 C14 9, 12 11, 10 14Z" fill="currentColor" opacity="0.7"/>`,

  // Chiller: snowflake
  chiller: `<line x1="10" y1="2" x2="10" y2="18" stroke="currentColor" stroke-width="1.5"/>
    <line x1="3" y1="6" x2="17" y2="14" stroke="currentColor" stroke-width="1.5"/>
    <line x1="3" y1="14" x2="17" y2="6" stroke="currentColor" stroke-width="1.5"/>
    <line x1="10" y1="2" x2="8" y2="4" stroke="currentColor" stroke-width="1"/>
    <line x1="10" y1="2" x2="12" y2="4" stroke="currentColor" stroke-width="1"/>
    <line x1="10" y1="18" x2="8" y2="16" stroke="currentColor" stroke-width="1"/>
    <line x1="10" y1="18" x2="12" y2="16" stroke="currentColor" stroke-width="1"/>`,

  // Cooling tower: tower shape with water drops
  cooling_tower: `<path d="M5 17 L7 5 L13 5 L15 17Z" fill="none" stroke="currentColor" stroke-width="1.5"/>
    <line x1="6" y1="5" x2="14" y2="5" stroke="currentColor" stroke-width="1.5"/>
    <path d="M8 3 Q8 1, 10 1 Q12 1, 12 3" fill="none" stroke="currentColor" stroke-width="1"/>
    <circle cx="9" cy="11" r="0.8" fill="currentColor"/>
    <circle cx="11" cy="9" r="0.8" fill="currentColor"/>
    <circle cx="10" cy="13" r="0.8" fill="currentColor"/>`,

  // Heat recovery: two arrows passing each other
  heat_recovery: `<path d="M3 7 L17 7" stroke="currentColor" stroke-width="1.5"/>
    <polygon points="15,5 17,7 15,9" fill="currentColor"/>
    <path d="M17 13 L3 13" stroke="currentColor" stroke-width="1.5"/>
    <polygon points="5,11 3,13 5,15" fill="currentColor"/>
    <line x1="10" y1="5" x2="10" y2="15" stroke="currentColor" stroke-width="1" stroke-dasharray="2 1"/>`,

  // Humidifier: water drops
  humidifier: `<path d="M10 3 C10 3, 5 9, 5 12 C5 15, 7 17, 10 17 C13 17, 15 15, 15 12 C15 9, 10 3, 10 3Z" fill="none" stroke="currentColor" stroke-width="1.5"/>
    <circle cx="8" cy="12" r="0.8" fill="currentColor"/>
    <circle cx="10" cy="10" r="0.8" fill="currentColor"/>
    <circle cx="12" cy="13" r="0.8" fill="currentColor"/>`,

  // Duct: straight pipe with flanges
  duct: `<rect x="2" y="7" width="16" height="6" rx="1" fill="none" stroke="currentColor" stroke-width="1.5"/>
    <line x1="2" y1="5" x2="2" y2="15" stroke="currentColor" stroke-width="1.5"/>
    <line x1="18" y1="5" x2="18" y2="15" stroke="currentColor" stroke-width="1.5"/>`,

  // Heat exchanger: two interleaved paths
  heat_exchanger: `<rect x="3" y="3" width="14" height="14" rx="2" fill="none" stroke="currentColor" stroke-width="1.5"/>
    <path d="M6 6 L14 14" stroke="currentColor" stroke-width="1.5"/>
    <path d="M14 6 L6 14" stroke="currentColor" stroke-width="1.5"/>
    <polygon points="13,5 15,6 13,7" fill="currentColor"/>
    <polygon points="7,13 5,14 7,15" fill="currentColor"/>`,

  // Coil load (water-side representation of a coil): coil symbol
  coil_load: `<path d="M4 6 Q7 4, 10 6 Q13 8, 16 6" fill="none" stroke="currentColor" stroke-width="1.5"/>
    <path d="M4 10 Q7 8, 10 10 Q13 12, 16 10" fill="none" stroke="currentColor" stroke-width="1.5"/>
    <path d="M4 14 Q7 12, 10 14 Q13 16, 16 14" fill="none" stroke="currentColor" stroke-width="1.5"/>`,

  // Evap cooler: water droplets + air flow arrow
  evap_cooler: `<path d="M2 10 L8 10" stroke="currentColor" stroke-width="1.5"/>
    <polygon points="7,7 11,10 7,13" fill="currentColor"/>
    <circle cx="14" cy="7" r="1.5" fill="none" stroke="currentColor" stroke-width="1.2"/>
    <path d="M14 9 Q14 11, 13 12" fill="none" stroke="currentColor" stroke-width="1"/>
    <circle cx="14" cy="13" r="1.5" fill="none" stroke="currentColor" stroke-width="1.2"/>
    <path d="M14 15 Q14 17, 13 18" fill="none" stroke="currentColor" stroke-width="1"/>
    <line x1="12" y1="3" x2="12" y2="17" stroke="currentColor" stroke-width="1" stroke-dasharray="2 1"/>`,

  // VRF outdoor unit: box with compressor symbol and heat exchange fins
  vrf_outdoor: `<rect x="2" y="4" width="16" height="12" rx="1" fill="none" stroke="currentColor" stroke-width="1.5"/>
    <circle cx="8" cy="10" r="3" fill="none" stroke="currentColor" stroke-width="1.2"/>
    <path d="M8 7 L8 8.5 M8 11.5 L8 13 M5 10 L6.5 10 M9.5 10 L11 10" stroke="currentColor" stroke-width="1"/>
    <line x1="13" y1="5" x2="13" y2="15" stroke="currentColor" stroke-width="1"/>
    <line x1="15" y1="5" x2="15" y2="15" stroke="currentColor" stroke-width="1"/>
    <line x1="17" y1="5" x2="17" y2="15" stroke="currentColor" stroke-width="1"/>`,

  // VRF indoor unit: wall-mount cassette shape with airflow arrows
  vrf_indoor: `<rect x="2" y="5" width="16" height="7" rx="1" fill="none" stroke="currentColor" stroke-width="1.5"/>
    <path d="M5 12 Q5 15, 3 17" fill="none" stroke="currentColor" stroke-width="1.2"/>
    <path d="M10 12 Q10 15, 10 17" fill="none" stroke="currentColor" stroke-width="1.2"/>
    <path d="M15 12 Q15 15, 17 17" fill="none" stroke="currentColor" stroke-width="1.2"/>
    <path d="M5 7 Q8 6, 10 7 Q12 8, 15 7" fill="none" stroke="currentColor" stroke-width="1"/>`,

  // Radiant panel: horizontal surface with wavy radiation lines below
  radiant_panel: `<rect x="2" y="4" width="16" height="3" rx="1" fill="currentColor" opacity="0.5"/>
    <rect x="2" y="4" width="16" height="3" rx="1" fill="none" stroke="currentColor" stroke-width="1.5"/>
    <path d="M5 10 Q6 12, 5 14" fill="none" stroke="currentColor" stroke-width="1.2"/>
    <path d="M10 10 Q11 12, 10 14" fill="none" stroke="currentColor" stroke-width="1.2"/>
    <path d="M15 10 Q16 12, 15 14" fill="none" stroke="currentColor" stroke-width="1.2"/>
    <line x1="3" y1="17" x2="17" y2="17" stroke="currentColor" stroke-width="1" stroke-dasharray="2 1"/>`,

  // Thermal storage: tank with ice/cold symbol
  thermal_storage: `<ellipse cx="10" cy="6" rx="7" ry="2.5" fill="none" stroke="currentColor" stroke-width="1.5"/>
    <line x1="3" y1="6" x2="3" y2="14" stroke="currentColor" stroke-width="1.5"/>
    <line x1="17" y1="6" x2="17" y2="14" stroke="currentColor" stroke-width="1.5"/>
    <ellipse cx="10" cy="14" rx="7" ry="2.5" fill="none" stroke="currentColor" stroke-width="1.5"/>
    <line x1="10" y1="8" x2="10" y2="12" stroke="currentColor" stroke-width="1.2"/>
    <line x1="7.5" y1="9" x2="12.5" y2="11" stroke="currentColor" stroke-width="1.2"/>
    <line x1="7.5" y1="11" x2="12.5" y2="9" stroke="currentColor" stroke-width="1.2"/>`,

  // GSHP: ground loop (wavy) + heat pump box
  gshp: `<rect x="6" y="2" width="8" height="7" rx="1" fill="none" stroke="currentColor" stroke-width="1.5"/>
    <path d="M9 4 Q8 5.5, 9 7" fill="none" stroke="currentColor" stroke-width="1"/>
    <path d="M11 4 Q12 5.5, 11 7" fill="none" stroke="currentColor" stroke-width="1"/>
    <line x1="10" y1="9" x2="10" y2="12" stroke="currentColor" stroke-width="1.5"/>
    <path d="M3 12 Q5 10, 7 12 Q9 14, 11 12 Q13 10, 15 12 Q17 14, 17 14" fill="none" stroke="currentColor" stroke-width="1.5"/>`,

  // External air: dashed boundary box with horizontal air-flow arrow through it
  external_air: `<rect x="2" y="5" width="16" height="10" rx="2" fill="none" stroke="currentColor" stroke-width="1.5" stroke-dasharray="3 2"/>
    <line x1="5" y1="10" x2="15" y2="10" stroke="currentColor" stroke-width="1.5"/>
    <polygon points="12,7 15,10 12,13" fill="currentColor"/>`,

  // External plant: dashed boundary box with vertical water-flow arrow through it
  external_plant: `<rect x="5" y="2" width="10" height="16" rx="2" fill="none" stroke="currentColor" stroke-width="1.5" stroke-dasharray="3 2"/>
    <line x1="10" y1="5" x2="10" y2="15" stroke="currentColor" stroke-width="1.5"/>
    <polygon points="7,12 10,15 13,12" fill="currentColor"/>`,
};

/** Render an HVAC icon as an SVG element string */
export function getHvacIconSvg(type: HvacNodeType): string {
  return ICONS[type] ?? ICONS.fan;
}
