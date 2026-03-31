# Multi-Material Decomposition (MMD) for Photon-Counting CT

## 1. Introduction

Multi-material decomposition (MMD) decomposes CT images into volume fraction maps of basis materials, enabling quantitative tissue characterization beyond simple Hounsfield Unit (HU) thresholding. This document describes the MMD method implemented in the PCAT Workstation v2 for pericoronary adipose tissue (PVAT) analysis using Siemens photon-counting CT (PCCT) mono-energetic data.

### 1.1 Clinical Motivation

Current PCAT analysis uses a single-energy fat attenuation index (FAI) computed by thresholding HU values in a [-190, -30] window. While effective for risk stratification, this approach cannot distinguish between the physical mechanisms driving HU changes in inflamed PVAT:

- **Edema** (increased water content) — raises HU
- **Lipolysis** (decreased lipid) — raises HU
- **Contrast enhancement** (iodine from neovascularization) — raises HU
- **Fibrosis** (collagen deposition, water-like attenuation) — raises HU

All four mechanisms increase HU, but they represent different pathophysiological processes. MMD separates these contributions quantitatively.

### 1.2 Available Data

- **Siemens NAEOTOM Alpha** photon-counting CT
- **Mono-energetic reconstructions:** 70, 100, 140, 150 keV
- **Polychromatic CCTA:** standard coronary CTA (can serve as 5th measurement)

---

## 2. Forward Model

### 2.1 Linear Attenuation Model

In image-domain material decomposition, the linear attenuation coefficient (LAC) $\mu_E$ at energy $E$ for a voxel containing a mixture of $T_0$ basis materials is modeled as:

$$\mu_E = \sum_{t=1}^{T_0} \mu_{tE} \, x_t$$

where:
- $\mu_{tE}$ is the LAC of the $t$-th basis material at energy $E$ (in mm$^{-1}$ or HU)
- $x_t$ is the volume fraction of the $t$-th basis material (dimensionless, $0 \leq x_t \leq 1$)

### 2.2 Volume Conservation

The volume fractions satisfy the sum-to-one constraint (volume conservation):

$$\sum_{t=1}^{T_0} x_t = 1, \quad x_t \geq 0 \;\; \forall t$$

### 2.3 Matrix Formulation

For a single pixel $p$ measured at $E$ energy levels with $T_0$ basis materials:

$$\vec{\mu}_p = A_0 \, \vec{x}_p$$

where:
- $\vec{\mu}_p = [\mu_{70}, \mu_{100}, \mu_{140}, \mu_{150}]^T$ is the $E \times 1$ measurement vector
- $A_0$ is the $E \times T_0$ material composition matrix
- $\vec{x}_p = [x_{\text{water}}, x_{\text{lipid}}, x_{\text{iodine}}]^T$ is the $T_0 \times 1$ volume fraction vector

For our system ($E = 4$ energies, $T_0 = 3$ materials):

$$A_0 = \begin{pmatrix} \mu_{\text{water},70} & \mu_{\text{lipid},70} & \mu_{\text{iodine},70} \\ \mu_{\text{water},100} & \mu_{\text{lipid},100} & \mu_{\text{iodine},100} \\ \mu_{\text{water},140} & \mu_{\text{lipid},140} & \mu_{\text{iodine},140} \\ \mu_{\text{water},150} & \mu_{\text{lipid},150} & \mu_{\text{iodine},150} \end{pmatrix}$$

This is an **overdetermined** system (4 equations, 3 unknowns).

---

## 3. Basis Materials

### 3.1 Chosen Basis: Water, Lipid, Iodine

| Material | Justification |
|----------|--------------|
| **Water** | Major component of soft tissue, edema, and inflammation. Serves as surrogate for fibrosis (collagen has water-like attenuation). |
| **Lipid** | Primary component of adipose tissue. Decreased lipid fraction indicates inflammation-driven lipolysis. |
| **Iodine** | Contrast agent with K-edge at 33.2 keV. Provides a spectrally independent 3rd dimension. Indicates neovascularization when present in PVAT. |

### 3.2 Why NOT Collagen

Collagen was considered as a 4th basis material but rejected for fundamental physics reasons:

1. **Spectral indistinguishability from water.** Collagen (Z_eff ~ 7.4-7.5) and water (Z_eff ~ 7.42) have nearly identical mass attenuation coefficient curves in the diagnostic energy range. Both are composed of low-Z elements (C, H, O, N) with no K-edge. The LAC difference is <1% at 100 keV.

2. **Only two independent attenuation processes.** X-ray attenuation in the diagnostic range is governed by photoelectric absorption ($\propto Z^{3-4} / E^3$) and Compton scattering ($\propto \rho_e$). This makes the space of attenuation curves fundamentally 2-dimensional for materials without K-edges (Alvarez & Macovski, 1976).

3. **Iodine enables 3-material decomposition.** Iodine's K-edge at 33.2 keV provides a genuinely independent spectral signature (sharp discontinuity in attenuation), enabling separation of a 3rd material. A 4th non-K-edge material would be linearly dependent on the first two.

4. **Water fraction is a fibrosis surrogate.** Collagen deposits displace lipid and have water-like CT attenuation. The water fraction map from 3-material decomposition already captures the signal that collagen would provide.

5. **Literature consensus.** AAPM Task Group 291 (McCollough et al., Med Phys 2020), Touch et al. (IEEE TMI 2021), and Phan et al. (PMC12060251) all confirm that reliable separation of 3+ non-K-edge materials from spectral CT is not achievable due to ill-conditioning.

### 3.3 LAC Calibration

The composition matrix $A_0$ requires LAC values for each basis material at each energy. Two approaches:

**ROI-based measurement (preferred):**
1. Select a uniform ROI in each mono-energetic image containing a known material
2. Water: blood pool or aorta (avoiding partial volume)
3. Lipid: subcutaneous adipose tissue
4. Iodine: contrast-enhanced vessel lumen (subtract water baseline)
5. Record the mean HU (or LAC) in each ROI at each energy

**NIST XCOM database:**
- Compute mass attenuation coefficients from elemental composition
- Multiply by material density to get LACs
- Reference: https://physics.nist.gov/PhysRefData/Xcom/html/xcom1.html

---

## 4. Solution Method

### 4.1 Why Not Regularization?

The reference papers (Niu 2014, Xue 2017, Xue 2021) employ iterative solvers with TV regularization, L$_0$ sparsity constraints, and edge-preserving penalties. These are **not needed** for our problem, for the following reasons:

| Paper | System | Why regularization was needed |
|-------|--------|-------------------------------|
| Niu 2014 | 2 energies, 2 materials | Ill-conditioned (spectral overlap between basis materials) |
| Xue 2017 | 2 energies, 3-5 materials | Underdetermined (more unknowns than equations) |
| Xue 2021 | 1 energy, 4-5 materials | Severely underdetermined (1 equation, many unknowns) |
| **Ours** | **4 energies, 3 materials** | **Overdetermined (more equations than unknowns)** |

Regularization serves to inject prior information into underdetermined or ill-conditioned systems. In our overdetermined system:

- **TV regularization** would introduce spatial bias and blur real tissue boundaries. It assumes piecewise-constant material images, which is not necessarily true in heterogeneous PVAT.
- **L$_0$ sparsity** was designed for SECT where each voxel could contain any combination from a large material dictionary. With only 3 basis materials and 4 energies, sparsity is implicit.
- **Siemens ADMIRE** iterative reconstruction already suppresses noise in the mono-energetic images. Additional TV smoothing would be double-denoising.
- The 4th energy measurement provides built-in noise averaging through overdetermination.

### 4.2 Weighted Least Squares (WLS)

The optimal estimator for the linear model $\vec{\mu}_p = A_0 \vec{x}_p$ with known noise statistics is the **weighted least squares** solution:

$$\hat{\vec{x}}_p = \arg\min_{\vec{x}} \; (\vec{\mu}_p - A_0 \vec{x})^T V^{-1} (\vec{\mu}_p - A_0 \vec{x})$$

where $V = \text{diag}(\sigma_{70}^2, \sigma_{100}^2, \sigma_{140}^2, \sigma_{150}^2)$ is the diagonal noise covariance matrix with per-energy noise variances.

The unconstrained solution is:

$$\hat{\vec{x}}_p^{\text{unconstrained}} = (A_0^T V^{-1} A_0)^{-1} A_0^T V^{-1} \vec{\mu}_p = P \, \vec{\mu}_p$$

where $P = (A_0^T V^{-1} A_0)^{-1} A_0^T V^{-1}$ is a $3 \times 4$ matrix that is **precomputed once** for all pixels.

This is the **Best Linear Unbiased Estimator (BLUE)** by the Gauss-Markov theorem, meaning it has minimum variance among all linear unbiased estimators.

### 4.3 Simplex Projection

The unconstrained WLS solution may violate the physical constraints ($\sum x_t = 1$, $x_t \geq 0$). We project onto the probability simplex:

$$\hat{\vec{x}}_p = \text{project\_simplex}(\hat{\vec{x}}_p^{\text{unconstrained}})$$

**Algorithm** (Duchi et al., 2008): Given $\vec{y} \in \mathbb{R}^n$, find $\vec{x}^* = \arg\min_{\vec{x} \in \Delta^{n-1}} \|\vec{x} - \vec{y}\|^2$ where $\Delta^{n-1} = \{\vec{x} : \sum x_i = 1, x_i \geq 0\}$:

1. Sort $\vec{y}$ in descending order: $y_{(1)} \geq y_{(2)} \geq \cdots \geq y_{(n)}$
2. Find $\hat{\rho} = \max\{j \in [n] : y_{(j)} + \frac{1}{j}(1 - \sum_{r=1}^{j} y_{(r)}) > 0\}$
3. Compute threshold: $\tau = \frac{1}{\hat{\rho}}(\sum_{r=1}^{\hat{\rho}} y_{(r)} - 1)$
4. Output: $x_i^* = \max(y_i - \tau, \; 0)$

This runs in $O(n \log n)$ time. For $n = 3$, it is effectively constant-time.

### 4.4 Fitting Residual

The per-pixel fitting residual quantifies decomposition quality:

$$r_p = \|A_0 \hat{\vec{x}}_p - \vec{\mu}_p\|_2$$

High residuals indicate voxels where the 3-material model is inadequate (e.g., bone, air, or metal artifacts).

### 4.5 HU Pre-filter

Before decomposition, voxels clearly outside the soft-tissue range are excluded:

$$\text{skip if} \quad \mu_{70,p} > 150 \;\text{HU} \quad \text{or} \quad \mu_{70,p} < -500 \;\text{HU}$$

This excludes bone/calcification (high HU) and air/lung (low HU) without needing them as basis materials.

---

## 5. Complete Algorithm

```
Input:
  volumes[4]:  mono-energetic CT images at 70, 100, 140, 150 keV (Array3<f32>)
  A₀:          4×3 material composition matrix (LACs of water, lipid, iodine at each energy)
  V:           4×4 diagonal noise covariance matrix

Precompute:
  P = (A₀ᵀ V⁻¹ A₀)⁻¹ A₀ᵀ V⁻¹     // 3×4 matrix, computed once

For each pixel p = (z, y, x) in parallel:
  // Gather measurements
  μ⃗ = [volumes[0][z,y,x], volumes[1][z,y,x], volumes[2][z,y,x], volumes[3][z,y,x]]

  // Pre-filter
  if μ⃗[0] > 150 or μ⃗[0] < -500:
    water[z,y,x] = 0; lipid[z,y,x] = 0; iodine[z,y,x] = 0
    continue

  // WLS solve
  x⃗ = P · μ⃗                          // 3×4 × 4×1 = 3×1

  // Simplex projection
  x⃗ = project_simplex(x⃗)            // enforce Σx=1, x≥0

  // Store results
  water[z,y,x]  = x⃗[0]
  lipid[z,y,x]  = x⃗[1]
  iodine[z,y,x] = x⃗[2]

  // Fitting residual
  residual[z,y,x] = ‖A₀·x⃗ - μ⃗‖₂

Output:
  water, lipid, iodine:  volume fraction maps (Array3<f32>)
  residual:              fitting quality map (Array3<f32>)
```

**Computational complexity:** $O(N)$ where $N$ is the number of voxels. Each pixel requires one 3×4 matrix-vector multiply (12 multiplications, 8 additions) plus simplex projection (constant for $n=3$).

**Expected performance:** ~1 second for a 512×512×200 volume using Rayon parallelism on a modern CPU.

---

## 6. Noise Analysis

### 6.1 Noise Propagation

The noise covariance of the decomposed material images is:

$$\text{Cov}(\hat{\vec{x}}_p) = P \, V \, P^T = (A_0^T V^{-1} A_0)^{-1}$$

This is a $3 \times 3$ matrix. The diagonal elements give the variance of each material fraction:

$$\text{var}(x_{\text{water}}) = [(A_0^T V^{-1} A_0)^{-1}]_{11}$$
$$\text{var}(x_{\text{lipid}}) = [(A_0^T V^{-1} A_0)^{-1}]_{22}$$
$$\text{var}(x_{\text{iodine}}) = [(A_0^T V^{-1} A_0)^{-1}]_{33}$$

Off-diagonal elements indicate cross-correlations between material fraction estimates.

### 6.2 Condition Number

The condition number of $A_0^T V^{-1} A_0$ quantifies the noise amplification:

$$\kappa = \frac{\lambda_{\max}(A_0^T V^{-1} A_0)}{\lambda_{\min}(A_0^T V^{-1} A_0)}$$

A low condition number (close to 1) indicates a well-conditioned decomposition. Our 4-energy system with iodine's K-edge should yield $\kappa < 100$, which is acceptable for clinical use.

---

## 7. Clinical Interpretation

### 7.1 Material Fraction Maps in PCAT

| Material | Normal PVAT | Inflamed PVAT | Interpretation |
|----------|-------------|---------------|----------------|
| Water | ~20-25% | ~30-37% | Edema + fibrosis (collagen is water-like) |
| Lipid | ~70-80% | ~55-65% | Lipolysis due to inflammatory cytokines |
| Iodine | ~0% | 0-5% | Neovascularization, contrast leakage |

### 7.2 Derived Biomarkers

- **Water fraction in PCAT VOI:** Direct inflammation biomarker (replaces FAI)
- **Lipid fraction gradient:** Radial profile of lipid content from vessel wall outward
- **Iodine uptake:** Binary indicator of neovascularization (iodine > threshold in PVAT)
- **Water/Lipid ratio:** Normalized inflammation index

---

## 8. References

1. Niu T, Dong X, Petrongolo M, Zhu L. "Iterative image-domain decomposition for dual-energy CT." *Med Phys.* 2014;41(4):041901.

2. Xue Y, Ruan R, Hu X, Kuang Y, Wang J, Long Y, Niu T. "Statistical image-domain multimaterial decomposition for dual-energy CT." *Med Phys.* 2017;44(3):886-901.

3. Xue Y, Qin W, Luo C, Yang P, Jiang Y, Tsui T, He H, Wang L, Qin J, Xie Y, Niu T. "Multi-material decomposition for single energy CT using material sparsity constraint." *IEEE Trans Med Imaging.* 2021;40(5):1303-1317.

4. Alvarez RE, Macovski A. "Energy-selective reconstructions in x-ray computerized tomography." *Phys Med Biol.* 1976;21(5):733-744.

5. McCollough CH, et al. "Principles and applications of multienergy CT: Report of AAPM Task Group 291." *Med Phys.* 2020;47(7):e881-e912.

6. Duchi J, Shalev-Shwartz S, Singer Y, Chandra T. "Efficient projections onto the l1-ball for learning in high dimensions." *ICML.* 2008;272-279.

7. Kotanidis CP, Antoniades C. "Perivascular fat imaging by CT: a virtual guide." *Br J Pharmacol.* 2021;178(22):4270-4290.

8. Touch M, et al. "On the conditioning of spectral channelization and its impact on multi-material decomposition." *IEEE Trans Med Imaging.* 2021;40(5):1301-1302.

9. Phan CM, et al. "Exploring bias in spectral CT material decomposition." *PMC.* 2025; PMC12060251.
