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

pub mod airflow_network;
pub mod convection;
pub mod ctf;
pub mod geometry;
pub mod ground_temp;
pub mod heat_balance;
pub mod infiltration;
pub mod internal_gains;
pub mod material;
pub mod schedule;
pub mod shading;
pub mod solar;
pub mod species;
pub mod surface;
pub mod zone;
pub mod zone_loads;

pub use airflow_network::{
    AirflowNetwork, AirflowNetworkConfig, CpFacade, CpModel, CpTable, LeakageClass,
    SurfaceAirflowOverride,
};
pub use geometry::{azimuth_to_cardinal, CardinalDirection, EnvelopeAreas, Point3D};
pub use ground_temp::GroundTempModel;
pub use heat_balance::{BuildingEnvelope, SolarDistributionMethod};
pub use infiltration::InfiltrationInput;
pub use internal_gains::InternalGainInput;
pub use material::{
    Construction, ConstructionLayer, FFactorConstruction, Material, ResolvedLayer,
    SimpleConstruction, WindowConstruction,
};
pub use schedule::{day_of_week, ScheduleInput, ScheduleManager};
pub use shading::{
    FinInput, OverhangInput, ShadingCalculation, ShadingSurfaceInput, WindowShadingInput,
};
pub use species::{SpeciesConfig, SpeciesGenerationInput, SpeciesTransport};
pub use surface::{BoundaryCondition, SurfaceInput, SurfaceType};
pub use zone::{
    dc_rack_inlet_max, DataCenterConfig, DuctLeakageInput, ExhaustFanInput, IdealLoadsAirSystem,
    InteriorSolarDistribution, OutdoorAirInput, RoomAirGradient, ThermostatScheduleEntry,
    VentilationScheduleEntry, ZoneInput,
};
pub use zone_loads::{
    EquipmentGainInput, ExhaustFanTopLevel, IdealLoadsTopLevel, InfiltrationInteraction,
    InfiltrationTopLevel, LightsInput, OutdoorAirTopLevel, PeopleInput, ThermostatInput,
    VentilationCombiningMethod, VentilationTopLevel,
};
