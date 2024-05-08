use crate::utils::{calc_create2_address, calc_create_address};
use crate::{LowLevelAPI, LowLevelSDK};
use alloc::vec;
use byteorder::{ByteOrder, LittleEndian};
use fluentbase_types::{
    Address, Bytes, Bytes32, ExitCode, B256, F254, KECCAK_EMPTY, NATIVE_TRANSFER_ADDRESS,
    NATIVE_TRANSFER_KECCAK, POSEIDON_EMPTY, U256,
};
use revm_primitives::AccountInfo;

/// Number of fields
pub const JZKT_ACCOUNT_FIELDS_COUNT: u32 = 6;
pub const JZKT_STORAGE_FIELDS_COUNT: u32 = 1;

pub const JZKT_ACCOUNT_BALANCE_FIELD: u32 = 0;
pub const JZKT_ACCOUNT_NONCE_FIELD: u32 = 1;
pub const JZKT_ACCOUNT_SOURCE_CODE_SIZE_FIELD: u32 = 2;
pub const JZKT_ACCOUNT_SOURCE_CODE_HASH_FIELD: u32 = 3;
pub const JZKT_ACCOUNT_RWASM_CODE_SIZE_FIELD: u32 = 4;
pub const JZKT_ACCOUNT_RWASM_CODE_HASH_FIELD: u32 = 5;

/// Compression flags for upper fields.
///
/// We compress following fields:
/// - balance (0) because of balance overflow
/// - source code hash (3) because its keccak256
///
/// Mask is: 0b00001001
pub const JZKT_ACCOUNT_COMPRESSION_FLAGS: u32 =
    (1 << JZKT_ACCOUNT_BALANCE_FIELD) + (1 << JZKT_ACCOUNT_SOURCE_CODE_HASH_FIELD);
pub const JZKT_STORAGE_COMPRESSION_FLAGS: u32 = 0;

pub type AccountCheckpoint = u64;
pub type AccountFields = [Bytes32; JZKT_ACCOUNT_FIELDS_COUNT as usize];

pub trait AccountManager {
    fn checkpoint(&self) -> AccountCheckpoint;
    fn commit(&self);
    fn rollback(&self, checkpoint: AccountCheckpoint);
    fn account(&self, address: Address) -> (Account, bool);
    fn write_account(&self, account: &Account);
    fn preimage_size(&self, hash: &[u8; 32]) -> u32;
    fn preimage(&self, hash: &[u8; 32]) -> Bytes;
    fn update_preimage(&self, key: &[u8; 32], field: u32, preimage: &[u8]);
    fn storage(&self, address: Address, slot: U256) -> (U256, bool);
    fn write_storage(&self, address: Address, slot: U256, value: U256) -> bool;
    fn log(&self, address: Address, data: Bytes, topics: &[B256]);
    fn exec_hash(
        &self,
        hash32_offset: *const u8,
        input: &[u8],
        fuel_offset: *mut u32,
        state: u32,
    ) -> (Bytes, i32);
}

#[derive(Debug, Clone)]
pub struct Account {
    pub address: Address,
    pub balance: U256,
    pub nonce: u64,
    pub source_code_size: u64,
    pub source_code_hash: B256,
    pub rwasm_code_size: u64,
    pub rwasm_code_hash: F254,
}

impl Into<AccountInfo> for Account {
    fn into(self) -> AccountInfo {
        AccountInfo {
            balance: self.balance,
            nonce: self.nonce,
            code_hash: self.source_code_hash,
            rwasm_code_hash: self.rwasm_code_hash,
            code: None,
            rwasm_code: None,
        }
    }
}

impl From<AccountInfo> for Account {
    fn from(value: AccountInfo) -> Self {
        Self {
            address: Address::ZERO,
            balance: value.balance,
            nonce: value.nonce,
            source_code_size: value
                .code
                .as_ref()
                .map(|v| v.len() as u64)
                .unwrap_or_default(),
            source_code_hash: value.code_hash,
            rwasm_code_size: value
                .rwasm_code
                .as_ref()
                .map(|v| v.len() as u64)
                .unwrap_or_default(),
            rwasm_code_hash: value.rwasm_code_hash,
        }
    }
}

impl Default for Account {
    fn default() -> Self {
        Self {
            address: Address::ZERO,
            rwasm_code_size: 0,
            source_code_size: 0,
            nonce: 0,
            balance: U256::ZERO,
            rwasm_code_hash: POSEIDON_EMPTY,
            source_code_hash: KECCAK_EMPTY,
        }
    }
}

impl Account {
    pub fn new(address: Address) -> Self {
        Self {
            address,
            ..Default::default()
        }
    }

    pub fn new_from_fields(address: Address, fields: &[Bytes32]) -> Self {
        let mut result = Self::new(address);
        assert_eq!(
            fields.len(),
            JZKT_ACCOUNT_FIELDS_COUNT as usize,
            "account fields len mismatch"
        );
        unsafe {
            result
                .balance
                .as_le_slice_mut()
                .copy_from_slice(&fields[JZKT_ACCOUNT_BALANCE_FIELD as usize]);
        }
        result.nonce = LittleEndian::read_u64(&fields[JZKT_ACCOUNT_NONCE_FIELD as usize]);
        result.source_code_size =
            LittleEndian::read_u64(&fields[JZKT_ACCOUNT_SOURCE_CODE_SIZE_FIELD as usize]);
        result
            .source_code_hash
            .copy_from_slice(&fields[JZKT_ACCOUNT_SOURCE_CODE_HASH_FIELD as usize]);
        result.rwasm_code_size =
            LittleEndian::read_u64(&fields[JZKT_ACCOUNT_RWASM_CODE_SIZE_FIELD as usize]);
        result
            .rwasm_code_hash
            .copy_from_slice(&fields[JZKT_ACCOUNT_RWASM_CODE_HASH_FIELD as usize]);
        result
    }

    pub fn get_fields(&self) -> AccountFields {
        let mut account_fields: AccountFields = Default::default();
        LittleEndian::write_u64(
            &mut account_fields[JZKT_ACCOUNT_RWASM_CODE_SIZE_FIELD as usize][..],
            self.rwasm_code_size,
        );
        LittleEndian::write_u64(
            &mut account_fields[JZKT_ACCOUNT_NONCE_FIELD as usize][..],
            self.nonce,
        );
        account_fields[JZKT_ACCOUNT_BALANCE_FIELD as usize]
            .copy_from_slice(&self.balance.as_le_slice());

        account_fields[JZKT_ACCOUNT_SOURCE_CODE_HASH_FIELD as usize]
            .copy_from_slice(self.source_code_hash.as_slice());
        account_fields[JZKT_ACCOUNT_RWASM_CODE_HASH_FIELD as usize]
            .copy_from_slice(self.rwasm_code_hash.as_slice());
        LittleEndian::write_u64(
            &mut account_fields[JZKT_ACCOUNT_SOURCE_CODE_SIZE_FIELD as usize][..],
            self.source_code_size,
        );
        account_fields
    }

    pub fn inc_nonce(&mut self) -> Result<u64, ExitCode> {
        let prev_nonce = self.nonce;
        self.nonce += 1;
        if self.nonce == u64::MAX {
            return Err(ExitCode::NonceOverflow);
        }
        Ok(prev_nonce)
    }

    #[deprecated(note = "use [write_account] method instead")]
    pub fn write_to_jzkt<AM: AccountManager>(&self, am: &AM) {
        am.write_account(self);
    }

    #[deprecated(note = "use [preimage] method instead")]
    pub fn load_source_bytecode<AM: AccountManager>(&self, am: &AM) -> Bytes {
        return am.preimage(&self.source_code_hash);
    }

    #[deprecated(note = "use [preimage] method instead")]
    pub fn load_rwasm_bytecode<AM: AccountManager>(&self, am: &AM) -> Bytes {
        return am.preimage(&self.rwasm_code_hash);
    }

    pub fn update_bytecode<AM: AccountManager>(
        &mut self,
        am: &AM,
        source_bytecode: &Bytes,
        source_hash: Option<B256>,
        rwasm_bytecode: &Bytes,
        rwasm_hash: Option<F254>,
    ) {
        let address_word = self.address.into_word();
        // calc source code hash (we use keccak256 for backward compatibility)
        self.source_code_hash = source_hash.unwrap_or_else(|| {
            LowLevelSDK::crypto_keccak256(
                source_bytecode.as_ptr(),
                source_bytecode.len() as u32,
                self.source_code_hash.as_mut_ptr(),
            );
            self.source_code_hash
        });
        self.source_code_size = source_bytecode.len() as u64;
        // calc rwasm code hash (we use poseidon function for rWASM bytecode)
        self.rwasm_code_hash = rwasm_hash.unwrap_or_else(|| {
            LowLevelSDK::crypto_poseidon(
                rwasm_bytecode.as_ptr(),
                rwasm_bytecode.len() as u32,
                self.rwasm_code_hash.as_mut_ptr(),
            );
            self.rwasm_code_hash
        });
        self.rwasm_code_size = rwasm_bytecode.len() as u64;
        // write all changes to database
        am.write_account(self);
        // make sure preimage of this hash is stored
        am.update_preimage(
            &address_word,
            JZKT_ACCOUNT_SOURCE_CODE_HASH_FIELD,
            source_bytecode.as_ref(),
        );
        am.update_preimage(
            &address_word,
            JZKT_ACCOUNT_RWASM_CODE_HASH_FIELD,
            rwasm_bytecode.as_ref(),
        );
    }

    pub fn create_account<AM: AccountManager>(
        am: &AM,
        caller: &mut Account,
        amount: U256,
        salt_hash: Option<(U256, B256)>,
    ) -> Result<Account, ExitCode> {
        // check if caller have enough balance
        if caller.balance < amount {
            return Err(ExitCode::InsufficientBalance);
        }
        // try to increment nonce
        let old_nonce = caller.inc_nonce()?;
        // calc address
        let callee_address = if let Some((salt, hash)) = salt_hash {
            calc_create2_address(&caller.address, &salt, &hash)
        } else {
            calc_create_address(&caller.address, old_nonce)
        };
        let (mut callee, _) = am.account(callee_address);
        // make sure there is no creation collision
        if callee.is_not_empty() {
            return Err(ExitCode::CreateCollision);
        }
        // change balance from caller and callee
        if let Err(exit_code) = Self::transfer(caller, &mut callee, amount) {
            return Err(exit_code);
        }
        // emit transfer log
        // Self::emit_transfer_log(&caller.address, &callee.address, &amount);
        // change nonce (we are always on spurious dragon)
        callee.nonce = 1;
        Ok(callee)
    }

    pub fn emit_transfer_log(_from: &Address, _to: &Address, _amount: &U256) {
        // let topics: [B256; 4] = [
        //     NATIVE_TRANSFER_KECCAK,
        //     from.into_word(),
        //     to.into_word(),
        //     B256::from(amount.to_be_bytes::<32>()),
        // ];
        // LowLevelSDK::jzkt_emit_log(
        //     NATIVE_TRANSFER_ADDRESS.as_ptr(),
        //     topics.as_ptr() as *const [u8; 32],
        //     4 * 32,
        //     core::ptr::null(),
        //     0,
        // );
    }

    pub fn sub_balance(&mut self, amount: U256) -> Result<(), ExitCode> {
        self.balance = self
            .balance
            .checked_sub(amount)
            .ok_or(ExitCode::InsufficientBalance)?;
        Ok(())
    }

    pub fn sub_balance_saturating(&mut self, amount: U256) {
        self.balance = self.balance.saturating_sub(amount);
    }

    pub fn add_balance(&mut self, amount: U256) -> Result<(), ExitCode> {
        self.balance = self
            .balance
            .checked_add(amount)
            .ok_or(ExitCode::OverflowPayment)?;
        Ok(())
    }

    pub fn add_balance_saturating(&mut self, amount: U256) {
        self.balance = self.balance.saturating_add(amount);
    }

    pub fn transfer(from: &mut Account, to: &mut Account, amount: U256) -> Result<(), ExitCode> {
        // update balances
        from.sub_balance(amount)?;
        to.add_balance(amount)?;
        Ok(())
    }

    #[inline(always)]
    pub fn is_not_empty(&self) -> bool {
        self.nonce != 0
            || self.source_code_hash != KECCAK_EMPTY
            || self.rwasm_code_hash != POSEIDON_EMPTY
    }
}