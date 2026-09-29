use warpui::{AppContext, Entity, SingletonEntity};

use super::search_item::SearchItem;
use crate::host_directory::{self, HostDirectoryModel};
use crate::search::command_palette::mixer::CommandPaletteItemAction;
use crate::search::data_source::{Query, QueryResult};
use crate::search::mixer::{DataSourceRunErrorWrapper, SyncDataSource};

/// Datasource that searches the servers in the server directory.
#[derive(Default)]
pub struct DataSource;

impl Entity for DataSource {
    type Event = ();
}

impl SyncDataSource for DataSource {
    type Action = CommandPaletteItemAction;

    fn run_query(
        &self,
        query: &Query,
        app: &AppContext,
    ) -> Result<Vec<QueryResult<Self::Action>>, DataSourceRunErrorWrapper> {
        let directory = HostDirectoryModel::as_ref(app);
        Ok(host_directory::search(directory.hosts(), &query.text)
            .into_iter()
            .map(|found| SearchItem::new(found.host, found.fuzzy))
            .map(QueryResult::from)
            .collect())
    }
}
