//! Box records in the controller's store, read and written with `machine-box`.

use moor_core::request::{MachineBox, MachineBoxOp, MachineBoxReq, Request};
use moor_core::view::user::MachineBoxValue;
use moor_core::BoxId;

use super::{Hooks, Record};

impl Hooks {
    /// Box `id`'s record, if it has one.
    pub(super) async fn load(&self, id: i64) -> Result<Option<Record>, String> {
        let op = MachineBoxOp::Get {
            box_id: BoxId::new(id),
        };
        let row = self.ask(op).await?.into_iter().next();
        row.map(|r| Record::from_row(&r).map(|(_, record)| record))
            .transpose()
    }

    /// Whether the controller says Box `id` is closed or gone.
    pub(crate) async fn is_closed(&self, id: i64) -> Result<bool, String> {
        let op = MachineBoxOp::Get {
            box_id: BoxId::new(id),
        };
        self.value(op).await.map(|v| v.closed)
    }

    /// Writes Box `id`'s record whole.
    pub(super) async fn keep(&self, id: i64, record: &Record) -> Result<(), String> {
        self.ask(MachineBoxOp::Put(record.to_row(id)?))
            .await
            .map(drop)
    }

    /// Drops Box `id`'s record.
    pub(super) async fn forget(&self, id: i64) -> Result<(), String> {
        let op = MachineBoxOp::Drop {
            box_id: BoxId::new(id),
        };
        self.ask(op).await.map(drop)
    }

    /// The records of the Boxes on machine `machine_id`, by Box id.
    pub(super) async fn on_machine(&self, machine_id: i64) -> Result<Vec<(i64, Record)>, String> {
        let rows = self.ask(MachineBoxOp::List { machine_id }).await?;
        rows.iter().map(Record::from_row).collect()
    }

    /// The Boxes with a record on a machine this runner holds a link to: only this runner can
    /// run their hooks, and another runner's machines are its to sweep.
    pub(crate) async fn recorded(&self) -> Result<Vec<i64>, String> {
        let machines = self.agents.as_ref().map(crate::Agents::online);
        let mut ids = Vec::new();
        for machine_id in machines.unwrap_or_default() {
            let records = self.on_machine(machine_id).await?;
            ids.extend(records.into_iter().map(|(id, _)| id));
        }
        Ok(ids)
    }

    /// Sends `machine-box` with `op`; the records it answers.
    async fn ask(&self, op: MachineBoxOp) -> Result<Vec<MachineBox>, String> {
        self.value(op).await.map(|v| v.boxes)
    }

    /// Sends `machine-box` with `op`; its whole answer.
    async fn value(&self, op: MachineBoxOp) -> Result<MachineBoxValue, String> {
        let client = self.client.clone();
        let request = Request::MachineBox(MachineBoxReq { op });
        let response = tokio::task::spawn_blocking(move || client.request(request, |_| {}))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| format!("the controller did not answer for the Box records: {e}"))?;
        if let Some(error) = response.error {
            return Err(format!(
                "the controller refused the Box records: {}",
                error.message
            ));
        }
        let value = response.value.unwrap_or_default();
        serde_json::from_value::<MachineBoxValue>(value)
            .map_err(|e| format!("the controller's Box records are malformed: {e}"))
    }
}

impl Record {
    /// The row that keeps this record for Box `id`.
    fn to_row(&self, id: i64) -> Result<MachineBox, String> {
        let json = |what: &str, v: Result<String, serde_json::Error>| {
            v.map_err(|e| format!("cannot write the Box's {what}: {e}"))
        };
        Ok(MachineBox {
            box_id: BoxId::new(id),
            machine_id: self.reach.machine_id,
            slot: self.slot,
            reach: json("reach", serde_json::to_string(&self.reach))?,
            facts: json("facts", serde_json::to_string(&self.facts))?,
            stage: self.stage,
            evicted: self.evicted,
        })
    }

    /// The Box id and record a row keeps.
    fn from_row(row: &MachineBox) -> Result<(i64, Self), String> {
        let id = row.box_id.get();
        let bad =
            |what: &str, e: serde_json::Error| format!("Box {id}'s record has bad {what}: {e}");
        let record = Self {
            reach: serde_json::from_str(&row.reach).map_err(|e| bad("reach", e))?,
            slot: row.slot,
            facts: serde_json::from_str(&row.facts).map_err(|e| bad("facts", e))?,
            stage: row.stage,
            evicted: row.evicted,
        };
        Ok((id, record))
    }
}
