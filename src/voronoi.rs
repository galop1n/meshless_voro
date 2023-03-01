use glam::DVec3;
#[cfg(feature = "rayon")]
use rayon::prelude::*;
#[cfg(feature = "hdf5")]
use std::error::Error;
#[cfg(feature = "hdf5")]
use std::path::Path;

use crate::rtree_nn::{build_rtree, nn_iter, wrapping_nn_iter};

pub use generator::Generator;
use voronoi_cell::ConvexCell;
pub use voronoi_cell::VoronoiCell;
pub use voronoi_face::VoronoiFace;

mod generator;
mod voronoi_cell;
mod voronoi_face;

#[derive(Clone, Copy)]
pub(crate) enum Dimensionality {
    Dimensionality1D,
    Dimensionality2D,
    Dimensionality3D,
}

impl From<usize> for Dimensionality {
    fn from(u: usize) -> Self {
        match u {
            1 => Dimensionality::Dimensionality1D,
            2 => Dimensionality::Dimensionality2D,
            3 => Dimensionality::Dimensionality3D,
            _ => panic!("Invalid Voronoi dimensionality!"),
        }
    }
}

impl From<Dimensionality> for usize {
    fn from(dimensionality: Dimensionality) -> Self {
        match dimensionality {
            Dimensionality::Dimensionality1D => 1,
            Dimensionality::Dimensionality2D => 2,
            Dimensionality::Dimensionality3D => 3,
        }
    }
}

/// The main Voronoi struct
pub struct Voronoi {
    anchor: DVec3,
    width: DVec3,
    cells: Vec<VoronoiCell>,
    faces: Vec<VoronoiFace>,
    cell_face_connections: Vec<usize>,
    dimensionality: Dimensionality,
}

impl Voronoi {
    /// Construct the Voronoi tesselation. This method runs in parallel if the `"rayon"` feature is enabled.
    ///
    /// Iteratively construct each Voronoi cell independently of each other by repeatedly clipping it by the nearest generators until a safety criterion is reached.
    /// For non-periodic Voronoi tesselations, all Voronoi cells are clipped by the simulation volume with given `anchor` and `width` if necessary.
    ///
    /// * `generators` - The seed points of the Voronoi cells.
    /// * `anchor` - The lower left corner of the simulation volume.
    /// * `width` - The width of the simulation volume. Also determines the period of periodic Voronoi tesselations.
    /// * `dimensionality` - The dimensionality of the Voronoi tesselation. The algorithm is mainly aimed at constructiong 3D Voronoi tesselations, but can be used for 1 or 2D as well.
    /// * `periodic` - Whether to apply periodic boundary conditions to the Voronoi tesselation.
    pub fn build(
        generators: &[DVec3],
        anchor: DVec3,
        width: DVec3,
        dimensionality: usize,
        periodic: bool,
    ) -> Self {
        let dimensionality = dimensionality.into();

        // Normalize the unused components of the simulation volume, so that the lower dimensional volumes will be correct.
        let mut this_anchor = anchor;
        let mut this_width = width;
        if periodic {
            this_anchor -= width;
            this_width *= 3.;
        }

        match dimensionality {
            Dimensionality::Dimensionality1D => {
                this_anchor.y = -0.5;
                this_anchor.z = -0.5;
                this_width.y = 1.;
                this_width.z = 1.
            }
            Dimensionality::Dimensionality2D => {
                this_anchor.z = -0.5;
                this_width.z = 1.;
            }
            _ => (),
        }

        let generators: Vec<Generator> = generators
            .iter()
            .enumerate()
            .map(|(id, &loc)| Generator::new(id, loc, dimensionality))
            .collect();

        let rtree = build_rtree(&generators);

        let mut faces: Vec<Vec<VoronoiFace>> = generators.iter().map(|_| vec![]).collect();
        #[cfg(feature = "rayon")]
        let cells = generators
            .par_iter()
            .zip(faces.par_iter_mut())
            .map(|(generator, faces)| {
                let mut convex_cell = ConvexCell::init(
                    generator.loc(),
                    this_anchor,
                    this_width,
                    generator.id(),
                    dimensionality,
                );
                let nearest_neighbours = if periodic {
                    wrapping_nn_iter(&rtree, generator.loc(), width, dimensionality)
                } else {
                    nn_iter(&rtree, generator.loc())
                };
                convex_cell.build(&generators, nearest_neighbours, dimensionality);
                VoronoiCell::from_convex_cell(&convex_cell, faces)
            })
            .collect();
        #[cfg(not(feature = "rayon"))]
        let cells = generators
            .iter()
            .zip(faces.iter_mut())
            .map(|(generator, faces)| {
                let mut convex_cell = ConvexCell::init(
                    generator.loc(),
                    this_anchor,
                    this_width,
                    generator.id(),
                    dimensionality,
                );
                let nearest_neighbours = if periodic {
                    wrapping_nn_iter(&rtree, generator.loc(), width, dimensionality)
                } else {
                    nn_iter(&rtree, generator.loc())
                };
                convex_cell.build(&generators, nearest_neighbours, dimensionality);
                VoronoiCell::from_convex_cell(&convex_cell, faces)
            })
            .collect();

        Voronoi {
            anchor: this_anchor,
            width: this_width,
            cells,
            faces: faces
                .into_iter()
                .flatten()
                .filter(|f| f.has_valid_dimensionality(dimensionality))
                .collect(),
            cell_face_connections: vec![],
            dimensionality,
        }
        .finalize()
    }

    /// Link the Voronoi faces to their respective cells.
    fn finalize(mut self) -> Self {
        let mut cell_face_connections: Vec<Vec<usize>> =
            (0..self.cells.len()).map(|_| vec![]).collect();

        for (i, face) in self.faces.iter().enumerate() {
            cell_face_connections[face.left()].push(i);
            if let (Some(right_idx), None) = (face.right(), face.shift()) {
                cell_face_connections[right_idx].push(i);
            }
        }

        let mut face_connections_offset = 0;
        for (i, cell) in self.cells.iter_mut().enumerate() {
            let face_count = cell_face_connections[i].len();
            cell.finalize(face_connections_offset, face_count);
            face_connections_offset += face_count;
        }

        self.cell_face_connections = cell_face_connections.into_iter().flatten().collect();

        self
    }

    /// The anchor of the simulation volume. All generators are assumed to be contained in this simulation volume.
    pub fn anchor(&self) -> DVec3 {
        self.anchor
    }

    /// The width of the simulation volume. All generators are assumed to be contained in this simulation volume.
    pub fn width(&self) -> DVec3 {
        self.width
    }

    /// Get the voronoi cells.
    pub fn cells(&self) -> &[VoronoiCell] {
        self.cells.as_ref()
    }

    /// Get the voronoi faces.
    pub fn faces(&self) -> &[VoronoiFace] {
        self.faces.as_ref()
    }

    /// Get a vector of the Voronoi faces by consuming the Voronoi struct.
    pub fn into_faces(self) -> Vec<VoronoiFace> {
        self.faces
    }

    /// Get the links between the cells and their faces.
    pub fn cell_face_connections(&self) -> &[usize] {
        self.cell_face_connections.as_ref()
    }

    pub fn dimensionality(&self) -> usize {
        self.dimensionality.into()
    }

    /// Save the Voronoi tesselation to a hdf5 file.
    #[cfg(feature = "hdf5")]
    pub fn save<P: AsRef<Path>>(&self, filename: P) -> Result<(), Box<dyn Error>> {
        // Create the file to write the data to
        let file = hdf5::File::create(filename)?;

        // Write cell info
        let group = file.create_group("Cells")?;
        let data = self.cells.iter().map(|c| c.volume()).collect::<Vec<_>>();
        group
            .new_dataset_builder()
            .with_data(&data)
            .create("Volume")?;
        let data = self
            .cells
            .iter()
            .map(|c| c.face_connections_offset())
            .collect::<Vec<_>>();
        group
            .new_dataset_builder()
            .with_data(&data)
            .create("FaceConnectionsOffset")?;
        let data = self
            .cells
            .iter()
            .map(|c| c.face_count())
            .collect::<Vec<_>>();
        group
            .new_dataset_builder()
            .with_data(&data)
            .create("FaceCount")?;
        let data = self
            .cells
            .iter()
            .map(|c| c.centroid().to_array())
            .collect::<Vec<_>>();
        group
            .new_dataset_builder()
            .with_data(&data)
            .create("Centroid")?;
        let data = self
            .cells
            .iter()
            .map(|c| c.loc().to_array())
            .collect::<Vec<_>>();
        group
            .new_dataset_builder()
            .with_data(&data)
            .create("Generator")?;

        // Write face info
        let group = file.create_group("Faces")?;
        let data = self.faces.iter().map(|f| f.area()).collect::<Vec<_>>();
        group
            .new_dataset_builder()
            .with_data(&data)
            .create("Area")?;
        let data = self
            .faces
            .iter()
            .map(|f| f.centroid().to_array())
            .collect::<Vec<_>>();
        group
            .new_dataset_builder()
            .with_data(&data)
            .create("Centroid")?;
        let data = self
            .faces
            .iter()
            .map(|f| f.normal().to_array())
            .collect::<Vec<_>>();
        group
            .new_dataset_builder()
            .with_data(&data)
            .create("Normal")?;
        if let Dimensionality::Dimensionality2D = self.dimensionality {
            // Also write face start and end points
            let face_directions = self
                .faces
                .iter()
                .map(|f| f.area() * f.normal().cross(DVec3::Z))
                .collect::<Vec<_>>();
            let face_start = self
                .faces
                .iter()
                .zip(face_directions.iter())
                .map(|(f, &d)| (f.centroid() - 0.5 * d).to_array())
                .collect::<Vec<_>>();
            let face_end = self
                .faces
                .iter()
                .zip(face_directions.iter())
                .map(|(f, &d)| (f.centroid() + 0.5 * d).to_array())
                .collect::<Vec<_>>();
            group
                .new_dataset_builder()
                .with_data(&face_start)
                .create("Start")?;
            group
                .new_dataset_builder()
                .with_data(&face_end)
                .create("End")?;
        }

        // Write cell face connections
        file.new_dataset_builder()
            .with_data(self.cell_face_connections())
            .create("CellFaceConnections")?;

        Ok(())
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use float_cmp::assert_approx_eq;
    use rand::{distributions::Uniform, prelude::*};

    const DIM2D: usize = 2;
    const DIM3D: usize = 3;

    fn perturbed_grid(anchor: DVec3, width: DVec3, count: usize, pert: f64) -> Vec<DVec3> {
        let mut generators = vec![];
        let mut rng = thread_rng();
        let distr = Uniform::new(-0.5, 0.5);
        for n in 0..count.pow(3) {
            let i = n / count.pow(2);
            let j = (n % count.pow(2)) / count;
            let k = n % count;
            generators.push(
                DVec3 {
                    x: i as f64 + 0.5 + pert * rng.sample(distr),
                    y: j as f64 + 0.5 + pert * rng.sample(distr),
                    z: k as f64 + 0.5 + pert * rng.sample(distr),
                } * width
                    / count as f64
                    + anchor,
            );
        }

        generators
    }

    fn perturbed_plane(anchor: DVec3, width: DVec3, count: usize, pert: f64) -> Vec<DVec3> {
        let mut generators = vec![];
        let mut rng = thread_rng();
        let distr = Uniform::new(-0.5, 0.5);
        for n in 0..count.pow(2) {
            let i = n / count;
            let j = n % count;
            generators.push(
                DVec3 {
                    x: i as f64 + 0.5 + pert * rng.sample(distr),
                    y: j as f64 + 0.5 + pert * rng.sample(distr),
                    z: 0.5 * count as f64,
                } * width
                    / count as f64
                    + anchor,
            );
        }

        generators
    }

    #[test]
    fn test_single_cell() {
        let generators = vec![DVec3::splat(0.5)];
        let anchor = DVec3::ZERO;
        let width = DVec3::splat(1.);
        let voronoi = Voronoi::build(&generators, anchor, width, DIM3D, false);
        assert_approx_eq!(f64, voronoi.cells[0].volume(), 1.);
    }

    #[test]
    fn test_two_cells() {
        let generators = vec![
            DVec3 {
                x: 0.3,
                y: 0.4,
                z: 0.25,
            },
            DVec3 {
                x: 0.7,
                y: 0.6,
                z: 0.75,
            },
        ];
        let anchor = DVec3::ZERO;
        let width = DVec3::splat(1.);
        let voronoi = Voronoi::build(&generators, anchor, width, DIM3D, false);
        assert_approx_eq!(f64, voronoi.cells[0].volume(), 0.5);
        assert_approx_eq!(f64, voronoi.cells[1].volume(), 0.5);
    }

    #[test]
    fn test_4_cells() {
        let generators = vec![
            DVec3 {
                x: 0.4,
                y: 0.3,
                z: 0.,
            },
            DVec3 {
                x: 1.6,
                y: 0.2,
                z: 0.,
            },
            DVec3 {
                x: 0.6,
                y: 0.8,
                z: 0.,
            },
            DVec3 {
                x: 1.4,
                y: 0.7,
                z: 0.,
            },
        ];
        let anchor = DVec3::ZERO;
        let width = DVec3 {
            x: 2.,
            y: 1.,
            z: 1.,
        };
        let voronoi = Voronoi::build(&generators, anchor, width, DIM2D, true);
        assert_approx_eq!(f64, voronoi.cells.iter().map(|c| c.volume()).sum(), 2.);
    }

    #[test]
    fn test_five_cells() {
        let delta = 0.1f64.sqrt();
        let generators = vec![
            DVec3 {
                x: 0.5,
                y: 0.5,
                z: 0.5,
            },
            DVec3 {
                x: 0.5 - delta,
                y: 0.5 - delta,
                z: 0.5,
            },
            DVec3 {
                x: 0.5 - delta,
                y: 0.5 + delta,
                z: 0.5,
            },
            DVec3 {
                x: 0.5 + delta,
                y: 0.5 + delta,
                z: 0.5,
            },
            DVec3 {
                x: 0.5 + delta,
                y: 0.5 - delta,
                z: 0.5,
            },
        ];
        let anchor = DVec3::ZERO;
        let width = DVec3::splat(1.);
        let voronoi = Voronoi::build(&generators, anchor, width, DIM2D, false);
        assert_approx_eq!(f64, voronoi.cells[0].volume(), 0.2);
        assert_approx_eq!(f64, voronoi.cells[1].volume(), 0.2);
        assert_approx_eq!(f64, voronoi.cells[2].volume(), 0.2);
        assert_approx_eq!(f64, voronoi.cells[3].volume(), 0.2);
        assert_approx_eq!(f64, voronoi.cells[4].volume(), 0.2);
    }

    #[test]
    fn test_eight_cells() {
        let anchor = DVec3::ZERO;
        let width = DVec3::splat(1.);
        let generators = perturbed_grid(anchor, width, 2, 0.);
        let voronoi = Voronoi::build(&generators, anchor, width, DIM3D, false);
        for cell in &voronoi.cells {
            assert_approx_eq!(f64, cell.volume(), 0.125);
        }
    }

    #[test]
    fn test_27_cells() {
        let anchor = DVec3::ZERO;
        let width = DVec3::splat(1.);
        let generators = perturbed_grid(anchor, width, 3, 0.);
        let voronoi = Voronoi::build(&generators, anchor, width, DIM3D, false);
        for cell in &voronoi.cells {
            assert_approx_eq!(f64, cell.volume(), 1. / 27.);
        }
    }

    #[test]
    fn test_64_cells() {
        let anchor = DVec3::ZERO;
        let width = DVec3::splat(1.);
        let generators = perturbed_grid(anchor, width, 4, 0.);
        let voronoi = Voronoi::build(&generators, anchor, width, DIM3D, false);
        for cell in &voronoi.cells {
            assert_approx_eq!(f64, cell.volume(), 1. / 64.);
        }
    }

    #[test]
    fn test_125_cells() {
        let pert = 0.5;
        let anchor = DVec3::ZERO;
        let width = DVec3::splat(1.);
        let generators = perturbed_grid(anchor, width, 5, pert);
        let voronoi = Voronoi::build(&generators, anchor, width, DIM3D, false);
        let mut total_volume = 0.;
        for cell in &voronoi.cells {
            total_volume += cell.volume();
        }
        assert_approx_eq!(f64, total_volume, 1., epsilon = 1e-10, ulps = 8)
    }

    #[test]
    fn test_2_d() {
        let pert = 0.95;
        let count = 50;
        let anchor = DVec3::splat(2.);
        let width = DVec3 {
            x: 2.,
            y: 2.,
            z: 1.,
        };
        let generators = perturbed_plane(anchor, width, count, pert);
        let voronoi = Voronoi::build(&generators, anchor, width, DIM2D, true);

        #[cfg(feature = "hdf5")]
        voronoi.save("test.hdf5").unwrap();

        assert_approx_eq!(
            f64,
            voronoi.cells.iter().map(|c| c.volume()).sum(),
            4.,
            epsilon = 1e-10,
            ulps = 8
        );
    }

    #[test]
    fn test_3_d() {
        let pert = 0.5;
        let count = 64;
        let anchor = DVec3::ZERO;
        let width = DVec3::splat(2.);
        let generators = perturbed_grid(anchor, width, count, pert);
        let voronoi = Voronoi::build(&generators, anchor, width, DIM3D, false);
        let total_volume: f64 = voronoi.cells.iter().map(|c| c.volume()).sum();
        assert_eq!(voronoi.cells.len(), generators.len());
        assert_approx_eq!(f64, total_volume, 8., epsilon = 1e-10, ulps = 8);
    }
}
