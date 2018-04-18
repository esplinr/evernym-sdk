use serde_json;
use serde_json::Value;
extern crate rand;

use settings;
use rand::Rng;
use std::fmt;
use std::sync::Mutex;
use std::string::ToString;
use std::collections::HashMap;
use utils::error;
use utils::types::SchemaKey;
use utils::constants::{ SCHEMA_REQ, CREATE_SCHEMA_RESULT, SCHEMA_TXN, SCHEMA_TYPE, SCHEMA_ID, SCHEMA_JSON };
use utils::libindy::{
    ledger::{
        libindy_build_get_txn_request,
        libindy_build_get_schema_request,
        libindy_submit_request,
        libindy_build_schema_request,
        libindy_parse_get_schema_response,
        libindy_sign_and_submit_request
    },
    pool::{ get_pool_handle },
    wallet::{ get_wallet_handle },
    anoncreds::libindy_issuer_create_schema
};
use error::schema::SchemaError;

lazy_static! {
    static ref SCHEMA_MAP: Mutex<HashMap<u32, Box<CreateSchema>>> = Default::default();
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct SchemaData {
    name: String,
    version: String,
    #[serde(rename = "attrNames")]
    attr_names: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LedgerSchema {
    pub schema_id: String,
    pub schema_json: String,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct CreateSchema {
    data: Vec<String>,
    version: String,
    schema_id: String,
    #[serde(skip_serializing, default)]
    handle: u32,
    name: String,
    source_id: String,
    sequence_num: u32,
}

pub trait Schema: ToString {
    type SchemaType;

    fn retrieve_schema(submitter_did: &str, schema_id: &str) -> Result<(String, String), SchemaError> {
        if settings::test_indy_mode_enabled() { return Ok((SCHEMA_ID.to_string(), SCHEMA_JSON.to_string()))}

        //Todo: Change SchemaError to InvalidSchemaId
        let get_schema_req = libindy_build_get_schema_request(submitter_did, schema_id)
            .or(Err(SchemaError::InvalidSchemaSeqNo()))?;

        let get_schema_response = libindy_submit_request(&get_schema_req)
            .map_err(|err| SchemaError::CommonError(err))?;

        libindy_parse_get_schema_response(&get_schema_response)
            .map_err(|err| SchemaError::CommonError(err))
    }

    fn create_schema(submitter_did: &str,
                      name: &str,
                      version: &str,
                      data: &str) -> Result<String, SchemaError> {
        if settings::test_indy_mode_enabled() { return Ok(SCHEMA_ID.to_string()) }

        let (id, create_schema) = libindy_issuer_create_schema(submitter_did, name, version, data)
            .or(Err(SchemaError::InvalidSchemaCreation()))?;

        let request = libindy_build_schema_request(submitter_did, &create_schema)
            .or(Err(SchemaError::InvalidSchemaCreation()))?;

        let response = libindy_sign_and_submit_request(submitter_did, &request)
            .or(Err(SchemaError::InvalidSchemaCreation()))?;

        Self::check_submit_schema_response(&response)?;

        Ok(id)
    }

    fn check_submit_schema_response(txn: &str) -> Result<(), SchemaError> {
        let txn_val:  Value = serde_json::from_str(txn)
            .or(Err(SchemaError::CommonError(error::INVALID_JSON.code_num)))?;

        println!("txn: {}", txn);
        match txn_val.get("result") {
            Some(_) => return Ok(()),
            None => warn!("No result found in ledger txn. Must be Rejectd"),
        };

        match txn_val.get("op") {
            Some(m) => {
                if m == "REJECT" {
                    match txn_val.get("reason") {
                        Some(r) => Err(SchemaError::DuplicateSchema(r.to_string())),
                        None => Err(SchemaError::UnknownRejection()),
                    }
                } else {
                    return Err(SchemaError::CommonError(error::INVALID_JSON.code_num))
                }},
            None => return Err(SchemaError::CommonError(error::INVALID_JSON.code_num))
        }
    }
}

impl Schema for LedgerSchema {
    type SchemaType = LedgerSchema;
}

impl Schema for CreateSchema {
    type SchemaType = CreateSchema;
}

impl fmt::Display for LedgerSchema {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        match serde_json::to_string(&self.schema_json ){
            Ok(s) => {
                write!(f, "{}", s)
            },
            Err(e) => {
                error!("{}: {:?}",error::INVALID_SCHEMA.message, e);
                write!(f, "null")
            }
        }
    }
}

impl fmt::Display for CreateSchema {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        match serde_json::to_string(&self){
            Ok(s) => {
                write!(f, "{}", s)
            },
            Err(e) => {
                error!("{}: {:?}",error::INVALID_SCHEMA.message, e);
                write!(f, "null")
            }
        }
    }
}

impl LedgerSchema {

    pub fn new_from_ledger(id: &str) -> Result<LedgerSchema, SchemaError>
    {
        //Todo: find out what submitter did needs to be
        let submitter_did = &settings::get_config_value(settings::CONFIG_INSTITUTION_DID).unwrap();
        let (schema_id, schema_json) = LedgerSchema::retrieve_schema(submitter_did, id)?;
        Ok(LedgerSchema{
            schema_id,
            schema_json,
        })
    }
}

impl CreateSchema {

    pub fn set_sequence_num(&mut self, sequence_num: u32) {self.sequence_num = sequence_num;}

    pub fn get_sequence_num(&self) -> u32 {let sequence_num = self.sequence_num as u32; sequence_num}

    pub fn get_source_id(&self) -> &String { &self.source_id }

    pub fn get_schema_id(&self) -> &String { &self.schema_id }

}

pub fn create_new_schema(source_id: &str,
                         issuer_did: String,
                         schema_name: String,
                         version: String,
                         data: String) -> Result<u32, SchemaError> {
    debug!("creating schema with source_id: {}, name: {}, issuer_did: {}", source_id, schema_name, issuer_did);
    let schema_id = LedgerSchema::create_schema(&issuer_did,
                                                &schema_name,
                                                &version,
                                                &data)?;

    debug!("created schema on ledger with id: {}", schema_id);

    let new_handle = rand::thread_rng().gen::<u32>();
    let mut new_schema = Box::new(CreateSchema {
        source_id: source_id.to_string(),
        handle: new_handle,
        name: schema_name,
        data: serde_json::from_str(&data).unwrap_or_default(),
        version,
        schema_id,
        //Todo: Take sequence number out. Id will be used instead
        sequence_num: 0,
    });

    {
        let mut m = SCHEMA_MAP.lock().unwrap();
        debug!("inserting handle {} into schema table", new_handle);
        m.insert(new_handle, new_schema);
    }

    Ok(new_handle)
}


pub fn get_schema_attrs(source_id: String, schema_id: String) -> Result<(u32, String), SchemaError> {
    //Todo: which did should I use for submitter did
    let submitter_did = settings::get_config_value(settings::CONFIG_INSTITUTION_DID).unwrap();
    let new_handle = rand::thread_rng().gen::<u32>();

    let (schema_id, schema_json) = LedgerSchema::retrieve_schema(&submitter_did, &schema_id)?;
    let schema_data: SchemaData = serde_json::from_str(&schema_json)
        .or(Err(SchemaError::CommonError(error::INVALID_JSON.code_num)))?;

    let new_schema = Box::new(CreateSchema {
        source_id,
        schema_id,
        sequence_num: 0,
        handle: new_handle,
        name: schema_data.name,
        version: schema_data.version,
        data: schema_data.attr_names,
    });

    {
        let mut m = SCHEMA_MAP.lock().unwrap();
        debug!("inserting handle {} into schema table", new_handle);
        m.insert(new_handle, new_schema);
    }
    Ok((new_handle, to_string(new_handle)?))
}

pub fn is_valid_handle(handle: u32) -> bool {
    match SCHEMA_MAP.lock().unwrap().get(&handle) {
        Some(_) => true,
        None => false,
    }
}

pub fn get_sequence_num(handle: u32) -> Result<u32, SchemaError> {
    match SCHEMA_MAP.lock().unwrap().get(&handle) {
        Some(x) => Ok(x.get_sequence_num()),
        None => Err(SchemaError::InvalidHandle()),
    }
}

pub fn to_string(handle: u32) -> Result<String, SchemaError> {
    match SCHEMA_MAP.lock().unwrap().get(&handle) {
        Some(p) => Ok(p.to_string().to_owned()),
        None => Err(SchemaError::InvalidHandle()),
    }
}

pub fn get_source_id(handle: u32) -> Result<String, u32> {
    match SCHEMA_MAP.lock().unwrap().get(&handle) {
        Some(s) => Ok(s.get_source_id().clone()),
        None => Err(error::INVALID_SCHEMA_HANDLE.code_num),
    }
}

pub fn get_schema_id(handle: u32) -> Result<String, u32> {
    match SCHEMA_MAP.lock().unwrap().get(&handle) {
        Some(s) => Ok(s.get_schema_id().clone()),
        None => Err(error::INVALID_SCHEMA_HANDLE.code_num),
    }
}

pub fn from_string(schema_data: &str) -> Result<u32, SchemaError> {
    let derived_schema: CreateSchema = serde_json::from_str(schema_data)
        .map_err(|_| {
            error!("Invalid Json format for CreateSchema string");
            SchemaError::CommonError(error::INVALID_JSON.code_num)
        })?;

    let new_handle = rand::thread_rng().gen::<u32>();
    let source_id = derived_schema.source_id.clone();
    let schema = Box::from(derived_schema);

    {
        let mut m = SCHEMA_MAP.lock().unwrap();
        debug!("inserting handle {} with source_id {:?} into schema table", new_handle, source_id);
        m.insert(new_handle, schema);
    }
    Ok(new_handle)
}

pub fn release(handle: u32) -> Result< u32, SchemaError> {
    match SCHEMA_MAP.lock().unwrap().remove(&handle) {
        Some(t) => Ok(error::SUCCESS.code_num),
        None => Err(SchemaError::InvalidHandle()),
    }
}

pub fn release_all() {
    let mut map = SCHEMA_MAP.lock().unwrap();

    map.drain();
}

#[cfg(test)]
mod tests {
    use super::*;
    use settings;
    use utils::libindy::pool;
    use utils::libindy::wallet::{ delete_wallet, init_wallet };
    use utils::error::INVALID_JSON;
    use error::ToErrorCode;

    static  EXAMPLE: &str = r#"{
    "seqNo": 15,
    "dest": "4fUDR9R7fjwELRvH9JT6HH",
    "identifier":"4fUDR9R7fjwELRvH9JT6HH",
    "txnTime": 1510246647,
    "type": "107",
    "data": {
       "version": "0.1",
       "name": "Home Address",
       "attr_names": [
         "address1",
         "address2",
         "city",
         "state",
         "zip"
       ]
    }
}"#;

    static DIRTY_EXAMPLE: &str = r#"
{
  "auditPath":[
    "ERHXC95c5GkeGN1Cn8AsFL8ruU65Mmc5948ey4FybZMk",
    "8RPu6xcwmSaEgVohv83GtZu2hjJm5ghWQ6UEvSdjYCg4",
    "FUUbzChmnGjrGChBv3LZoKunodBPrVuMcg2vUrhkndmz"
  ],
  "data":{
    "attr_names":[
      "address1",
      "address2",
      "city",
      "state",
      "zip"
    ],
    "name":"Home Address",
    "version":"0.1"
  },
  "identifier":"4fUDR9R7fjwELRvH9JT6HH",
  "reqId":1510246647859168767,
  "rootHash":"Gnrip4cJgwJ3HE1fbrTBAPcuJ9RejAhX12PAUaF5HMij",
  "seqNo":15,
  "signature":"2paGvrWEfsCAYFAD47Qh7hedinymLy8VsbfatUrjWW7tpcryFtTsikJjWhKkD5QA3PLr7dLTmBFteNr4LWRHhrEn",
  "txnTime":1510246647,
  "type":"101"
}"#;
    static BAD_LEDGER_SAMPLE: &str = r#"{"result":{}"#;
    static LEDGER_SAMPLE: &str = r#"
        {
          "result":{
            "data":{
              "rootHash":"Gnrip4cJgwJ3HE1fbrTBAPcuJ9RejAhX12PAUaF5HMij",
              "data":{
                "version":"0.1",
                "name":"Home Address",
                "attr_names":[
                  "address1",
                  "address2",
                  "city",
                  "state",
                  "zip"
                ]
              },
              "reqId":1510246647859168767,
              "seqNo":15,
              "txnTime":1510246647,
              "signature":"2paGvrWEfsCAYFAD47Qh7hedinymLy8VsbfatUrjWW7tpcryFtTsikJjWhKkD5QA3PLr7dLTmBFteNr4LWRHhrEn",
              "type":"101",
              "identifier":"4fUDR9R7fjwELRvH9JT6HH",
              "auditPath":[
                "ERHXC95c5GkeGN1Cn8AsFL8ruU65Mmc5948ey4FybZMk",
                "8RPu6xcwmSaEgVohv83GtZu2hjJm5ghWQ6UEvSdjYCg4",
                "FUUbzChmnGjrGChBv3LZoKunodBPrVuMcg2vUrhkndmz"
              ]
            },
            "type":"3",
            "identifier":"GGBDg1j8bsKmr4h5T9XqYf",
            "reqId":1513364428103873981,
            "seqNo":15
          },
          "op":"REPLY"
        }
        "#;

    static  EXAMPLE_OPTIONAL: &str = r#"{
}"#;

    #[test]
    fn test_ledger_schema_to_string(){
        let schema = LedgerSchema {schema_json: "".to_string(), schema_id: "".to_string()};
        println!("{}", schema.to_string())
    }

    #[test]
    fn test_create_schema_to_string(){
        let create_schema = CreateSchema {
            data: vec!["name".to_string(), "age".to_string(), "sex".to_string(), "height".to_string()],
            version: "1.0".to_string(),
            schema_id: SCHEMA_ID.to_string(),
            source_id: "testId".to_string(),
            handle: 1,
            name: "schema_name".to_string(),
            sequence_num: 306,
        };
        println!("{}", create_schema.to_string());
        let create_schema_str = r#"{"data":["name","age","sex","height"],"version":"1.0","schema_id":"2hoqvcwupRTUNkXn6ArYzs:2:schema_name:0.0.11","name":"schema_name","source_id":"testId","sequence_num":306}"#;
        assert_eq!(create_schema.to_string(), create_schema_str.to_string());
    }

    #[test]
    fn test_create_schema_success(){
        settings::set_config_value(settings::CONFIG_ENABLE_TEST_MODE, "true");
        let data = r#"["name","male"]"#;
        assert!(create_new_schema("1",
                                  "VsKV7grR1BUE29mG2Fm2kX".to_string(),
                                  "name".to_string(),
                                  "1.0".to_string(),
                                  data.to_string()).is_ok());
    }

    #[test]
    fn test_get_schema_attrs_success(){
        settings::set_defaults();
        settings::set_config_value(settings::CONFIG_ENABLE_TEST_MODE, "true");
        let (handle, schema_attrs ) = get_schema_attrs("Check For Success".to_string(), SCHEMA_ID.to_string()).unwrap();
        assert!(schema_attrs.contains(r#""schema_id":"2hoqvcwupRTUNkXn6ArYzs:2:schema_name:0.0.11""#));
        assert!(schema_attrs.contains(r#""data":["sex","age","name","height"]"#));
        assert!(handle > 0);
    }

    #[test]
    fn test_create_schema_fails(){
        settings::set_defaults();
        settings::set_config_value(settings::CONFIG_ENABLE_TEST_MODE, "false");
        let schema = create_new_schema("1", "VsKV7grR1BUE29mG2Fm2kX".to_string(),
                                       "name".to_string(),
                                       "1.0".to_string(),
                                       "".to_string());

        assert_eq!(schema.err(),Some(SchemaError::InvalidSchemaCreation()));
    }

    #[ignore]
    #[test]
    fn test_get_schema_attrs_from_ledger(){
        settings::set_defaults();
        pool::open_sandbox_pool();
        let data = r#""data":{"name":"New Credential - Credential5","version":"1.0","attr_names":["New Credential","credential5","a5","b5","c5","d5"]}"#.to_string();
        init_wallet("test_get_schema_attrs_from_ledger").unwrap();
        let wallet_handle = get_wallet_handle();
        let (_, schema_attrs ) = get_schema_attrs("id".to_string(), SCHEMA_ID.to_string()).unwrap();
        assert!(schema_attrs.contains(r#""version":"0.0.1""#));
        assert!(schema_attrs.contains(r#""schema_id":"2hoqvcwupRTUNkXn6ArYzs:2:schema_name:0.0.1""#));
        delete_wallet("test_get_schema_attrs_from_ledger").unwrap();
    }

    #[ignore]
    #[test]
    fn test_create_schema(){
        settings::set_defaults();
        settings::set_config_value(settings::CONFIG_ENABLE_TEST_MODE, "false");
        ::utils::devsetup::setup_dev_env("test_create_schema");

        let data = r#"["address1","address2","zip","city","state"]"#.to_string();
        //Todo: use new version number
        let version = r#"0.0.314"#.to_string();
        let did = r#"2hoqvcwupRTUNkXn6ArYzs"#.to_string();
        let handle = create_new_schema("id", did, "name".to_string(), version,data).unwrap();
        delete_wallet("test_create_schema").unwrap();
        assert!(handle > 0);
        let rc = get_schema_id(handle);
        assert!(rc.is_ok());
    }


    #[ignore]
    #[test]
    fn test_create_duplicate_fails(){
        settings::set_defaults();
        settings::set_config_value(settings::CONFIG_ENABLE_TEST_MODE, "false");
        ::utils::devsetup::setup_dev_env("test_create_schema");

        let data = r#"["address1","address2","zip","city","state"]"#.to_string();
        let version = r#"0.0.2"#.to_string();
        let did = r#"2hoqvcwupRTUNkXn6ArYzs"#.to_string();
        let rc = create_new_schema("id", did, "name".to_string(), version,data);
        delete_wallet("test_create_schema").unwrap();
        assert!(rc.is_err());
    }

    #[ignore]
    #[test]
    fn from_pool_ledger_with_id(){
        //Todo: Add to integration tests so that its not ignored
        settings::set_defaults();
        settings::set_config_value(settings::CONFIG_ENABLE_TEST_MODE, "false");
        pool::open_sandbox_pool();
        let schema_id = r#"2hoqvcwupRTUNkXn6ArYzs:2:schema_nam:2.2.2"#;
        let expected_schema_data = r#"{"ver":"1.0","id":"2hoqvcwupRTUNkXn6ArYzs:2:schema_nam:2.2.2","name":"schema_nam","version":"2.2.2","attrNames":["sex","age","name","height"],"seqNo":1659}"#;
        let (id, retrieved_schema) = LedgerSchema::retrieve_schema("2hoqvcwupRTUNkXn6ArYzs", schema_id).unwrap();
        assert!(retrieved_schema.contains(r#""ver":"1.0","id":"2hoqvcwupRTUNkXn6ArYzs:2:schema_nam:2.2.2","name":"schema_nam","version":"2.2.2""#));
    }

    #[test]
    fn from_ledger_schema_id(){
        settings::set_defaults();
        settings::set_config_value(settings::CONFIG_ENABLE_TEST_MODE, "true");
        let (id, retrieved_schema) = LedgerSchema::retrieve_schema(SCHEMA_ID, "2hoqvcwupRTUNkXn6ArYzs").unwrap();
        assert_eq!(&retrieved_schema, SCHEMA_JSON);
        assert_eq!(&id, SCHEMA_ID);
    }

    #[test]
    fn test_release_all() {
        settings::set_defaults();
        settings::set_config_value(settings::CONFIG_ENABLE_TEST_MODE, "true");
        let data = r#"["address1","address2","zip","city","state"]"#;
        let version = r#"0.0.0"#;
        let did = r#"2hoqvcwupRTUNkXn6ArYzs"#;
        let h1 = create_new_schema("1", did.to_string(), "name".to_string(), version.to_string(),data.to_string()).unwrap();
        let h2 = create_new_schema("1", did.to_string(), "name".to_string(), version.to_string(),data.to_string()).unwrap();
        let h3 = create_new_schema("1", did.to_string(), "name".to_string(), version.to_string(),data.to_string()).unwrap();
        let h4 = create_new_schema("1", did.to_string(), "name".to_string(), version.to_string(),data.to_string()).unwrap();
        let h5 = create_new_schema("1", did.to_string(), "name".to_string(), version.to_string(),data.to_string()).unwrap();
        release_all();
        assert_eq!(release(h1).err(),Some(SchemaError::InvalidHandle()));
        assert_eq!(release(h2).err(),Some(SchemaError::InvalidHandle()));
        assert_eq!(release(h3).err(),Some(SchemaError::InvalidHandle()));
        assert_eq!(release(h4).err(),Some(SchemaError::InvalidHandle()));
        assert_eq!(release(h5).err(),Some(SchemaError::InvalidHandle()));
    }

    #[test]
    fn test_errors(){
        settings::set_defaults();
        settings::set_config_value(settings::CONFIG_ENABLE_TEST_MODE, "false");
        assert_eq!(get_sequence_num(145661).err(), Some(SchemaError::InvalidHandle()));
        assert_eq!(to_string(13435178).err(), Some(SchemaError::InvalidHandle()));
        let test: Result<LedgerSchema, SchemaError> = LedgerSchema::new_from_ledger(SCHEMA_ID);
        // This error will throw when run outside of all the other test modules, but will NOT
        // error when a pool is open from any previous test.  Ideally we fix this by closing our
        // opened pools.
//        use utils::error::NO_POOL_OPEN;
//        assert_eq!(test.err(), Some(SchemaError::CommonError(NO_POOL_OPEN.code_num)));
        let bad_schema = EXAMPLE;
        assert_eq!(from_string(bad_schema).err(), Some(SchemaError::CommonError(INVALID_JSON.code_num)));
    }

    #[test]
    fn test_extract_data_from_schema_json() {
        let data: SchemaData = serde_json::from_str(SCHEMA_JSON).unwrap();
        assert_eq!(data.name, "schema_name".to_string());
    }
}
