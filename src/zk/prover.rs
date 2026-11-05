use std::path::Path;

use anyhow::format_err;
use halo2_base::{
    gates::circuit::{BaseCircuitParams, CircuitBuilderStage, builder::BaseCircuitBuilder},
    gates::flex_gate::MultiPhaseThreadBreakPoints,
    halo2_proofs::{
        halo2curves::bn256::Bn256,
        plonk::{ProvingKey, create_proof, keygen_pk, keygen_vk_with_k},
        poly::{gwc_kzg::GwcKZGCommitmentScheme, kzg::params::ParamsKZG},
        transcript::{CircuitTranscript, Transcript},
    },
};
use halo2_solana_verifier::{
    curve::{G1, G2},
    kzg::KzgVk,
    verify_gwc,
};
use halo2_solana_vk_host::compile_vk;
use rand::rngs::OsRng;
use shielded_pool_circuit::{
    circuit::{
        full_circuit::build_full_circuit,
        prover::{BLINDING_FACTOR, CIRCUIT_K, ProverInput, build_fixture_input_for_step, load_srs},
    },
    transcript::{SolanaKeccak, fr_to_be},
};

use super::field::{Fr, TREE_DEPTH};
use crate::app_error::{AppError, RawResult, RequestResult};

/// Circuit VK the program embeds (`solana-proof-generator/fixtures/vk.bin`).
const PINNED_VK: &[u8] = include_bytes!("../../resources/vk.bin");

const G1_LEN: usize = 64;
const G2_LEN: usize = 128;

/// Trimmed KZG VK the program embeds: `[1]_1 || [1]_2 || [tau]_2`.
const PINNED_KZG_VK: &[u8; G1_LEN + 2 * G2_LEN] = include_bytes!("../../resources/kzg_vk.bin");

/// Public inputs in circuit order: step, chunk_amount, dest_address, nullifier, root.
pub const PUBLIC_INPUTS_LEN: usize = 5;

type GwcKzg = GwcKZGCommitmentScheme<Bn256>;

/// Proving key and circuit layout, built once at startup.
pub struct ProvingContext {
    params: ParamsKZG<Bn256>,
    proving_key: ProvingKey<Fr, GwcKzg>,
    circuit_params: BaseCircuitParams,
    break_points: MultiPhaseThreadBreakPoints,
    pinned_kzg_vk: KzgVk,
}

pub struct GeneratedProof {
    pub proof: Vec<u8>,
    /// BE, as the circuit computed them, in circuit order.
    pub public_inputs_be: [[u8; 32]; PUBLIC_INPUTS_LEN],
}

impl ProvingContext {
    /// Runs keygen and checks the circuit VK equals the program VK.
    pub fn load(srs_path: &Path) -> RawResult<Self> {
        let params = load_srs(srs_path).map_err(|load_srs_error| {
            format_err!(
                "cannot load the SRS from SRS_PATH {} ({load_srs_error}): run scripts/fetch-srs.sh",
                srs_path.display()
            )
        })?;

        // Keygen needs any valid witness: the fixture one.
        let keygen_circuit_builder = BaseCircuitBuilder::from_stage(CircuitBuilderStage::Keygen)
            .use_k(CIRCUIT_K as usize)
            .use_instance_columns(1)
            .use_lookup_bits(CIRCUIT_K as usize - 1);

        let mut keygen_circuit =
            create_circuit(keygen_circuit_builder, &build_fixture_input_for_step(0));

        let circuit_params = keygen_circuit.calculate_params(Some(BLINDING_FACTOR));

        let verifying_key = keygen_vk_with_k(&params, &keygen_circuit, CIRCUIT_K)
            .map_err(|keygen_error| format_err!("keygen_vk_with_k: {keygen_error:?}"))?;

        let proving_key = keygen_pk(verifying_key, &keygen_circuit)
            .map_err(|keygen_error| format_err!("keygen_pk: {keygen_error:?}"))?;

        let compiled_vk = compile_vk(&params, proving_key.get_vk())
            .map_err(|compile_error| format_err!("compile_vk: {compile_error:?}"))?;

        if compiled_vk != PINNED_VK {
            return Err(format_err!(
                "the client circuit VK differs from the program VK in resources/vk.bin: \
                 copy vk.bin and kzg_vk.bin from the solana-proof-generator commit in Cargo.lock"
            ));
        }

        Ok(Self {
            params,
            proving_key,
            circuit_params,
            break_points: keygen_circuit.break_points(),
            pinned_kzg_vk: create_pinned_kzg_vk(),
        })
    }

    /// Proof blinded with `OsRng` and verified against the program VKs.
    pub fn prove(&self, prover_input: &ProverInput<TREE_DEPTH>) -> RequestResult<GeneratedProof> {
        let prover_circuit_builder =
            BaseCircuitBuilder::prover(self.circuit_params.clone(), self.break_points.clone());

        let prover_circuit = create_circuit(prover_circuit_builder, prover_input);

        let public_values: Vec<Fr> = prover_circuit.assigned_instances[0]
            .iter()
            .map(|assigned_value| *assigned_value.value())
            .collect();

        let mut transcript = CircuitTranscript::<SolanaKeccak>::init();
        create_proof::<Fr, GwcKzg, _, _>(
            &self.params,
            &self.proving_key,
            &[prover_circuit],
            &[&[public_values.as_slice()]],
            OsRng,
            &mut transcript,
        )
        .map_err(|proof_error| AppError::ProverError(format!("create_proof: {proof_error:?}")))?;
        let proof = transcript.finalize();

        let public_inputs_be: [[u8; 32]; PUBLIC_INPUTS_LEN] = public_values
            .iter()
            .map(fr_to_be)
            .collect::<Vec<_>>()
            .try_into()
            .expect("the circuit has PUBLIC_INPUTS_LEN public inputs");

        let accepted = verify_gwc(PINNED_VK, &proof, &public_inputs_be, &self.pinned_kzg_vk)
            .map_err(|verify_error| {
                AppError::ProverError(format!("verify_gwc: {verify_error:?}"))
            })?;

        if !accepted {
            return Err(AppError::ProverError(
                "the proof does not verify against the program VK".into(),
            ));
        }

        Ok(GeneratedProof {
            proof,
            public_inputs_be,
        })
    }
}

/// Copy of the private `circuit::prover::build_circuit`, solana-proof-generator commit 02153b91ea9a60bc39b8aee473166c6789aa48d0.
fn create_circuit(
    mut circuit_builder: BaseCircuitBuilder<Fr>,
    prover_input: &ProverInput<TREE_DEPTH>,
) -> BaseCircuitBuilder<Fr> {
    build_full_circuit::<TREE_DEPTH>(
        &mut circuit_builder,
        prover_input.s,
        prover_input.total_amount,
        &prover_input.chunks,
        &prover_input.addresses,
        &prover_input.siblings_path,
        &prover_input.siblings_side,
        prover_input.step,
        prover_input.chunk_amount,
        prover_input.dest_address,
        prover_input.nullifier,
    );
    circuit_builder
}

/// Splits `kzg_vk.bin` the same way the program does.
fn create_pinned_kzg_vk() -> KzgVk {
    let (g1_one, g2_points) = PINNED_KZG_VK.split_at(G1_LEN);
    let (g2_one, g2_tau) = g2_points.split_at(G2_LEN);

    KzgVk {
        g1_one: G1(g1_one.try_into().expect("longer than G1_LEN bytes")),
        g2_one: G2(g2_one.try_into().expect("longer than G2_LEN bytes")),
        g2_tau: G2(g2_tau.try_into().expect("longer than G2_LEN bytes")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zk::field::CHUNKS;

    const SRS_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/resources/kzg_bn254_16.srs");

    const FIXTURE_PUBLIC_INPUTS: [&[u8; 32 * PUBLIC_INPUTS_LEN]; CHUNKS] = [
        include_bytes!("../../tests/fixtures/step0/public_inputs.bin"),
        include_bytes!("../../tests/fixtures/step1/public_inputs.bin"),
        include_bytes!("../../tests/fixtures/step2/public_inputs.bin"),
    ];

    /// Keygen matches the program VK, every fixture step proves to the fixture public inputs and
    /// passes `verify_gwc` (inside `prove`), and blinding is fresh on every proof.
    #[test]
    #[ignore = "slow: keygen and 5 proofs; needs resources/kzg_bn254_16.srs (scripts/fetch-srs.sh)"]
    fn proves_the_fixture_steps_with_fresh_blinding() {
        let proving_context = ProvingContext::load(Path::new(SRS_PATH)).unwrap();

        for (step, fixture_public_inputs) in FIXTURE_PUBLIC_INPUTS.iter().enumerate() {
            let generated_proof = proving_context
                .prove(&build_fixture_input_for_step(step))
                .unwrap();

            assert_eq!(
                generated_proof.public_inputs_be.concat(),
                fixture_public_inputs.as_slice(),
                "step {step}"
            );
        }

        let first_proof = proving_context
            .prove(&build_fixture_input_for_step(0))
            .unwrap();

        let second_proof = proving_context
            .prove(&build_fixture_input_for_step(0))
            .unwrap();

        assert_ne!(
            first_proof.proof, second_proof.proof,
            "blinding must come from OsRng"
        );
    }
}
