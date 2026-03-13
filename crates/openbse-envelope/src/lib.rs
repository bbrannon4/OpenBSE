//! Building envelope heat balance and surface models.
//!
//! Implements the thermal physics of the building shell:
//! - CTF (Conduction Transfer Functions) for opaque surface conduction
//! - Zone air heat balance (predictor-corrector)
//! - Solar processing (position, incident, transmitted)
//! - Convection coefficients (interior ASHRAE simple, exterior TARP)
//! - Infiltration (design flow rate model)
//! - Internal gains (people, lights, equipment)
//! - Vertex-based geometry (auto-calculating area, azimuth, tilt)
//! - Ground temperature model (Kusuda-Achenbach)

pub mod material;
pub mod surface;
pub mod zone;
pub mod ctf;
pub mod solar;
pub mod convection;
pub mod infiltration;
pub mod internal_gains;
pub mod schedule;
pub mod heat_balance;
pub mod geometry;
pub mod ground_temp;
pub mod zone_loads;
pub mod shading;

pub use heat_balance::{BuildingEnvelope, SolarDistributionMethod};
pub use material::{Material, Construction, ConstructionLayer, ResolvedLayer, WindowConstruction, SimpleConstruction, FFactorConstruction};
pub use zone_loads::{PeopleInput, LightsInput, EquipmentGainInput, InfiltrationTopLevel,
    VentilationTopLevel, VentilationCombiningMethod, ExhaustFanTopLevel, OutdoorAirTopLevel,
    IdealLoadsTopLevel, ThermostatInput};
pub use surface::{SurfaceInput, SurfaceType, BoundaryCondition};
pub use zone::{ZoneInput, IdealLoadsAirSystem, ThermostatScheduleEntry,
    VentilationScheduleEntry, InteriorSolarDistribution, ExhaustFanInput, OutdoorAirInput};
pub use infiltration::InfiltrationInput;
pub use internal_gains::InternalGainInput;
pub use schedule::{ScheduleInput, ScheduleManager, day_of_week};
pub use geometry::{Point3D, CardinalDirection, EnvelopeAreas, azimuth_to_cardinal};
pub use ground_temp::GroundTempModel;
pub use shading::{ShadingSurfaceInput, WindowShadingInput, OverhangInput, FinInput, ShadingCalculation};
