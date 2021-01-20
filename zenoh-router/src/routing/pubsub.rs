//
// Copyright (c) 2017, 2020 ADLINK Technology Inc.
//
// This program and the accompanying materials are made available under the
// terms of the Eclipse Public License 2.0 which is available at
// http://www.eclipse.org/legal/epl-2.0, or the Apache License, Version 2.0
// which is available at https://www.apache.org/licenses/LICENSE-2.0.
//
// SPDX-License-Identifier: EPL-2.0 OR Apache-2.0
//
// Contributors:
//   ADLINK zenoh team, <zenoh@adlink-labs.tech>
//
use async_std::sync::Arc;
use petgraph::graph::NodeIndex;
use std::collections::HashMap;
use uhlc::HLC;

use zenoh_protocol::core::{
    whatami, CongestionControl, PeerId, Reliability, ResKey, SubInfo, SubMode, ZInt,
};
use zenoh_protocol::io::RBuf;
use zenoh_protocol::proto::DataInfo;

use crate::routing::face::FaceState;
use crate::routing::resource::{Context, Resource};
use crate::routing::router::Tables;

pub type DataRoute = HashMap<usize, (Arc<FaceState>, ResKey)>;

async fn propagate_simple_subscription(
    tables: &mut Tables,
    src_face: &mut Arc<FaceState>,
    res: &Arc<Resource>,
    sub_info: &SubInfo,
) {
    for dst_face in &mut tables.faces.values_mut() {
        if src_face.id != dst_face.id
            && !dst_face.local_subs.contains(res)
            && match tables.whatami {
                whatami::ROUTER => dst_face.whatami == whatami::CLIENT,
                whatami::PEER => dst_face.whatami == whatami::CLIENT,
                _ => (src_face.whatami == whatami::CLIENT || dst_face.whatami == whatami::CLIENT),
            }
        {
            unsafe {
                Arc::get_mut_unchecked(dst_face)
                    .local_subs
                    .push(res.clone());
                let reskey = Resource::decl_key(res, dst_face).await;
                dst_face
                    .primitives
                    .subscriber(&reskey, sub_info, None)
                    .await;
            }
        }
    }
}

async unsafe fn register_router_subscription(
    tables: &mut Tables,
    face: &mut Arc<FaceState>,
    res: &mut Arc<Resource>,
    sub_info: &SubInfo,
    router: PeerId,
) {
    if !res.router_subs.contains(&router) {
        // Register router subscription
        {
            let res_mut = Arc::get_mut_unchecked(res);
            log::debug!(
                "Register router subscription {} (router: {})",
                res_mut.name(),
                router
            );
            res_mut.router_subs.insert(router.clone());
            tables.router_subs.insert(res.clone());
        }

        // Propagate subscription to routers
        let net = tables.routers_net.as_ref().unwrap();
        match net.get_idx(&router) {
            Some(tree_sid) => {
                for child in &net.trees[tree_sid.index()].childs {
                    match tables.get_face(&net.graph[*child].pid).cloned() {
                        Some(mut someface) => {
                            if someface.id != face.id {
                                let reskey = Resource::decl_key(res, &mut someface).await;

                                log::debug!(
                                    "Send router subscription {} on face {} {}",
                                    res.name(),
                                    someface.id,
                                    someface.pid,
                                );

                                someface
                                    .primitives
                                    .subscriber(&reskey, &sub_info, Some(tree_sid.index() as ZInt))
                                    .await;
                            }
                        }
                        None => {
                            log::error!("Unable to find face for pid {}", net.graph[*child].pid)
                        }
                    }
                }
            }
            None => log::error!(
                "Error propagating sub {}: cannot get index of {}!",
                res.name(),
                router
            ),
        }

        // Propagate subscription to peers
        if face.whatami != whatami::PEER {
            register_peer_subscription(tables, face, res, sub_info, tables.pid.clone()).await
        }
    }

    // Propagate subscription to clients
    propagate_simple_subscription(tables, face, res, sub_info).await;
}

pub async fn declare_router_subscription(
    tables: &mut Tables,
    face: &mut Arc<FaceState>,
    prefixid: ZInt,
    suffix: &str,
    sub_info: &SubInfo,
    router: PeerId,
) {
    match tables.get_mapping(&face, &prefixid).cloned() {
        Some(mut prefix) => unsafe {
            let mut res = Resource::make_resource(&mut prefix, suffix);
            Resource::match_resource(&tables, &mut res);
            register_router_subscription(tables, face, &mut res, sub_info, router).await;

            Tables::build_matches_direct_tables(&mut res);
        },
        None => log::error!("Declare router subscription for unknown rid {}!", prefixid),
    }
}

async unsafe fn register_peer_subscription(
    tables: &mut Tables,
    face: &mut Arc<FaceState>,
    res: &mut Arc<Resource>,
    sub_info: &SubInfo,
    peer: PeerId,
) {
    if !res.peer_subs.contains(&peer) {
        // Register peer subscription
        {
            let res_mut = Arc::get_mut_unchecked(res);
            log::debug!(
                "Register peer subscription {} (peer: {})",
                res_mut.name(),
                peer
            );
            res_mut.peer_subs.insert(peer.clone());
            tables.peer_subs.insert(res.clone());
        }

        // Propagate subscription to peers
        let net = tables.peers_net.as_ref().unwrap();
        match net.get_idx(&peer) {
            Some(tree_sid) => {
                for child in &net.trees[tree_sid.index()].childs {
                    match tables.get_face(&net.graph[*child].pid).cloned() {
                        Some(mut someface) => {
                            if someface.id != face.id {
                                let reskey = Resource::decl_key(res, &mut someface).await;

                                log::debug!(
                                    "Send peer subscription {} on face {} {}",
                                    res.name(),
                                    someface.id,
                                    someface.pid,
                                );

                                someface
                                    .primitives
                                    .subscriber(&reskey, &sub_info, Some(tree_sid.index() as ZInt))
                                    .await;
                            }
                        }
                        None => {
                            log::error!("Unable to find face for pid {}", net.graph[*child].pid)
                        }
                    }
                }
            }
            None => log::error!(
                "Error propagating sub {}: cannot get index of {}!",
                res.name(),
                peer,
            ),
        }
    }
}

pub async fn declare_peer_subscription(
    tables: &mut Tables,
    face: &mut Arc<FaceState>,
    prefixid: ZInt,
    suffix: &str,
    sub_info: &SubInfo,
    peer: PeerId,
) {
    match tables.get_mapping(&face, &prefixid).cloned() {
        Some(mut prefix) => unsafe {
            let mut res = Resource::make_resource(&mut prefix, suffix);
            Resource::match_resource(&tables, &mut res);
            register_peer_subscription(tables, face, &mut res, sub_info, peer).await;

            if tables.whatami == whatami::ROUTER {
                let mut propa_sub_info = sub_info.clone();
                propa_sub_info.mode = SubMode::Push;
                register_router_subscription(
                    tables,
                    face,
                    &mut res,
                    &propa_sub_info,
                    tables.pid.clone(),
                )
                .await;
            }

            Tables::build_matches_direct_tables(&mut res);
        },
        None => log::error!("Declare router subscription for unknown rid {}!", prefixid),
    }
}

async unsafe fn register_client_subscription(
    _tables: &mut Tables,
    face: &mut Arc<FaceState>,
    res: &mut Arc<Resource>,
    sub_info: &SubInfo,
) {
    // Register subscription
    {
        let res = Arc::get_mut_unchecked(res);
        log::debug!("Register subscription {} for face {}", res.name(), face.id);
        match res.contexts.get_mut(&face.id) {
            Some(mut ctx) => match &ctx.subs {
                Some(info) => {
                    if SubMode::Pull == info.mode {
                        Arc::get_mut_unchecked(&mut ctx).subs = Some(sub_info.clone());
                    }
                }
                None => {
                    Arc::get_mut_unchecked(&mut ctx).subs = Some(sub_info.clone());
                }
            },
            None => {
                res.contexts.insert(
                    face.id,
                    Arc::new(Context {
                        face: face.clone(),
                        local_rid: None,
                        remote_rid: None,
                        subs: Some(sub_info.clone()),
                        qabl: false,
                        last_values: HashMap::new(),
                    }),
                );
            }
        }
    }
    Arc::get_mut_unchecked(face).remote_subs.push(res.clone());
}

pub async fn declare_client_subscription(
    tables: &mut Tables,
    face: &mut Arc<FaceState>,
    prefixid: ZInt,
    suffix: &str,
    sub_info: &SubInfo,
) {
    match tables.get_mapping(&face, &prefixid).cloned() {
        Some(mut prefix) => unsafe {
            let mut res = Resource::make_resource(&mut prefix, suffix);
            Resource::match_resource(&tables, &mut res);

            register_client_subscription(tables, face, &mut res, sub_info).await;
            match tables.whatami {
                whatami::ROUTER => {
                    let mut propa_sub_info = sub_info.clone();
                    propa_sub_info.mode = SubMode::Push;
                    register_router_subscription(
                        tables,
                        face,
                        &mut res,
                        &propa_sub_info,
                        tables.pid.clone(),
                    )
                    .await;
                }
                whatami::PEER => {
                    let mut propa_sub_info = sub_info.clone();
                    propa_sub_info.mode = SubMode::Push;
                    register_peer_subscription(
                        tables,
                        face,
                        &mut res,
                        &propa_sub_info,
                        tables.pid.clone(),
                    )
                    .await;
                }
                _ => {
                    propagate_simple_subscription(tables, face, &res, sub_info).await;
                }
            }

            Tables::build_matches_direct_tables(&mut res);
        },
        None => log::error!("Declare subscription for unknown rid {}!", prefixid),
    }
}

async unsafe fn propagate_forget_simple_subscription(tables: &mut Tables, res: &mut Arc<Resource>) {
    for face in tables.faces.values_mut() {
        if face.local_subs.contains(res) {
            let reskey = Resource::get_best_key(res, "", face.id);
            face.primitives.forget_subscriber(&reskey, None).await;

            Arc::get_mut_unchecked(face)
                .local_subs
                .retain(|sub| sub != res);
        }
    }
}

async unsafe fn unregister_router_subscription(
    tables: &mut Tables,
    face: &mut Arc<FaceState>,
    res: &mut Arc<Resource>,
    router: PeerId,
) {
    if res.router_subs.contains(&router) {
        log::debug!(
            "Unregister router subscription {} (router: {})",
            res.name(),
            router
        );
        Arc::get_mut_unchecked(res)
            .router_subs
            .retain(|sub| *sub != router);

        // Propagate forget subscription to routers
        let net = tables.routers_net.as_ref().unwrap();
        match net.get_idx(&router) {
            Some(tree_sid) => {
                for child in &net.trees[tree_sid.index()].childs {
                    match tables.get_face(&net.graph[*child].pid).cloned() {
                        Some(mut someface) => {
                            if someface.id != face.id {
                                let reskey = Resource::decl_key(res, &mut someface).await;

                                log::debug!(
                                    "Send forget router subscription {} on face {} {}",
                                    res.name(),
                                    someface.id,
                                    someface.pid,
                                );

                                someface
                                    .primitives
                                    .forget_subscriber(&reskey, Some(tree_sid.index() as ZInt))
                                    .await;
                            }
                        }
                        None => {
                            log::error!("Unable to find face for pid {}", net.graph[*child].pid)
                        }
                    }
                }
            }
            None => log::error!(
                "Error propagating sub {}: cannot get index of {}!",
                res.name(),
                router
            ),
        }

        if res.router_subs.is_empty() {
            tables.router_subs.retain(|sub| !Arc::ptr_eq(sub, &res));

            unregister_peer_subscription(tables, face, res, tables.pid.clone()).await;
            propagate_forget_simple_subscription(tables, res).await;
        }
    }
}

pub async fn undeclare_router_subscription(
    tables: &mut Tables,
    face: &mut Arc<FaceState>,
    prefixid: ZInt,
    suffix: &str,
    router: PeerId,
) {
    match tables.get_mapping(&face, &prefixid) {
        Some(prefix) => match Resource::get_resource(prefix, suffix) {
            Some(mut res) => unsafe {
                unregister_router_subscription(tables, face, &mut res, router).await;
                Resource::clean(&mut res)
            },
            None => log::error!("Undeclare unknown router subscription!"),
        },
        None => log::error!("Undeclare router subscription with unknown prefix!"),
    }
}

async unsafe fn unregister_peer_subscription(
    tables: &mut Tables,
    face: &mut Arc<FaceState>,
    res: &mut Arc<Resource>,
    peer: PeerId,
) {
    if res.peer_subs.contains(&peer) {
        log::debug!(
            "Unregister peer subscription {} (peer: {})",
            res.name(),
            peer
        );
        Arc::get_mut_unchecked(res)
            .peer_subs
            .retain(|sub| *sub != peer);

        // Propagate forget subscription to peers
        let net = tables.peers_net.as_ref().unwrap();
        match net.get_idx(&peer) {
            Some(tree_sid) => {
                for child in &net.trees[tree_sid.index()].childs {
                    match tables.get_face(&net.graph[*child].pid).cloned() {
                        Some(mut someface) => {
                            if someface.id != face.id {
                                let reskey = Resource::decl_key(res, &mut someface).await;

                                log::debug!(
                                    "Send forget peer subscription {} on face {} {}",
                                    res.name(),
                                    someface.id,
                                    someface.pid,
                                );

                                someface
                                    .primitives
                                    .forget_subscriber(&reskey, Some(tree_sid.index() as ZInt))
                                    .await;
                            }
                        }
                        None => {
                            log::error!("Unable to find face for pid {}", net.graph[*child].pid)
                        }
                    }
                }
            }
            None => log::error!(
                "Error propagating forget sub {}: cannot get index of {}!",
                res.name(),
                peer
            ),
        }

        if res.peer_subs.is_empty() {
            tables.peer_subs.retain(|sub| !Arc::ptr_eq(sub, &res));
        }
    }
}

pub async fn undeclare_peer_subscription(
    tables: &mut Tables,
    face: &mut Arc<FaceState>,
    prefixid: ZInt,
    suffix: &str,
    peer: PeerId,
) {
    match tables.get_mapping(&face, &prefixid) {
        Some(prefix) => match Resource::get_resource(prefix, suffix) {
            Some(mut res) => unsafe {
                unregister_peer_subscription(tables, face, &mut res, peer).await;

                if tables.whatami == whatami::ROUTER
                    && !res.contexts.values().any(|ctx| ctx.subs.is_some())
                    && !tables
                        .peer_subs
                        .iter()
                        .any(|res| res.peer_subs.iter().any(|peer| *peer != tables.pid))
                {
                    unregister_router_subscription(tables, face, &mut res, tables.pid.clone())
                        .await;
                }

                Resource::clean(&mut res)
            },
            None => log::error!("Undeclare unknown peer subscription!"),
        },
        None => log::error!("Undeclare peer subscription with unknown prefix!"),
    }
}

pub(crate) async unsafe fn unregister_client_subscription(
    tables: &mut Tables,
    face: &mut Arc<FaceState>,
    res: &mut Arc<Resource>,
) {
    log::debug!(
        "Unregister client subscription {} for face {}",
        res.name(),
        face.id
    );
    if let Some(mut ctx) = Arc::get_mut_unchecked(res).contexts.get_mut(&face.id) {
        Arc::get_mut_unchecked(&mut ctx).subs = None;
    }
    Arc::get_mut_unchecked(face)
        .remote_subs
        .retain(|x| !Arc::ptr_eq(&x, &res));

    match tables.whatami {
        whatami::ROUTER => {
            if !res.contexts.values().any(|ctx| ctx.subs.is_some())
                && !tables
                    .peer_subs
                    .iter()
                    .any(|res| res.peer_subs.iter().any(|peer| *peer != tables.pid))
            {
                unregister_router_subscription(tables, face, res, tables.pid.clone()).await;
            }
        }
        whatami::PEER => {
            if !res.contexts.values().any(|ctx| ctx.subs.is_some())
                && !tables
                    .peer_subs
                    .iter()
                    .any(|res| res.peer_subs.iter().any(|peer| *peer != tables.pid))
            {
                unregister_peer_subscription(tables, face, res, tables.pid.clone()).await;
            }
        }
        _ => {
            if !res.contexts.values().any(|ctx| ctx.subs.is_some()) {
                propagate_forget_simple_subscription(tables, res).await;
            }
        }
    }

    let mut client_subs: Vec<Arc<FaceState>> = res
        .contexts
        .values()
        .filter_map(|ctx| {
            if ctx.subs.is_some() {
                Some(ctx.face.clone())
            } else {
                None
            }
        })
        .collect();
    if client_subs.len() == 1
        && !tables
            .router_subs
            .iter()
            .any(|res| res.peer_subs.iter().any(|peer| *peer != tables.pid))
        && !tables
            .peer_subs
            .iter()
            .any(|res| res.peer_subs.iter().any(|peer| *peer != tables.pid))
    {
        let face = &mut client_subs[0];
        if face.local_subs.contains(&res) {
            let reskey = Resource::get_best_key(&res, "", face.id);
            face.primitives.forget_subscriber(&reskey, None).await;

            Arc::get_mut_unchecked(face)
                .local_subs
                .retain(|sub| sub != &(*res));
        }
    }

    Resource::clean(res)
}

pub async fn undeclare_client_subscription(
    tables: &mut Tables,
    face: &mut Arc<FaceState>,
    prefixid: ZInt,
    suffix: &str,
) {
    match tables.get_mapping(&face, &prefixid) {
        Some(prefix) => match Resource::get_resource(prefix, suffix) {
            Some(mut res) => unsafe {
                unregister_client_subscription(tables, face, &mut res).await;
            },
            None => log::error!("Undeclare unknown subscription!"),
        },
        None => log::error!("Undeclare subscription with unknown prefix!"),
    }
}

pub(crate) async fn pubsub_new_client_face(tables: &mut Tables, face: &mut Arc<FaceState>) {
    let sub_info = SubInfo {
        reliability: Reliability::Reliable, // TODO
        mode: SubMode::Push,
        period: None,
    };
    for sub in &tables.router_subs {
        unsafe {
            Arc::get_mut_unchecked(face).local_subs.push(sub.clone());
            let reskey = Resource::decl_key(&sub, face).await;
            face.primitives.subscriber(&reskey, &sub_info, None).await;
        }
    }
}

pub(crate) async fn pubsub_new_childs(
    tables: &mut Tables,
    childs: Vec<Vec<NodeIndex>>,
    net_type: whatami::Type,
) {
    for (tree_sid, tree_childs) in childs.into_iter().enumerate() {
        if !tree_childs.is_empty() {
            let net = match net_type {
                whatami::ROUTER => tables.routers_net.as_ref().unwrap(),
                _ => tables.peers_net.as_ref().unwrap(),
            };
            let tree_id = net.graph[NodeIndex::new(tree_sid)].pid.clone();

            let subs_res = match net_type {
                whatami::ROUTER => &tables.router_subs,
                _ => &tables.peer_subs,
            };

            for res in subs_res {
                let subs = match net_type {
                    whatami::ROUTER => &res.router_subs,
                    _ => &res.peer_subs,
                };
                for sub in subs {
                    if *sub == tree_id {
                        for child in &tree_childs {
                            match tables.get_face(&net.graph[*child].pid).cloned() {
                                Some(mut face) => {
                                    let reskey = Resource::decl_key(&res, &mut face).await;
                                    let sub_info = SubInfo {
                                        // TODO
                                        reliability: Reliability::Reliable,
                                        mode: SubMode::Push,
                                        period: None,
                                    };
                                    log::debug!(
                                        "Send {} subscription {} on face {} {} (new_child)",
                                        net_type,
                                        res.name(),
                                        face.id,
                                        face.pid,
                                    );
                                    face.primitives
                                        .subscriber(&reskey, &sub_info, Some(tree_sid as ZInt))
                                        .await;
                                }
                                None => {
                                    log::error!(
                                        "Unable to find face for pid {}",
                                        net.graph[*child].pid
                                    )
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[inline]
fn propagate_data(
    whatami: whatami::Type,
    src_face: &Arc<FaceState>,
    dst_face: &Arc<FaceState>,
) -> bool {
    src_face.id != dst_face.id
        && match whatami {
            whatami::ROUTER => {
                (src_face.whatami != whatami::PEER || dst_face.whatami != whatami::PEER)
                    && (src_face.whatami != whatami::ROUTER || dst_face.whatami != whatami::ROUTER)
            }
            _ => (src_face.whatami == whatami::CLIENT || dst_face.whatami == whatami::CLIENT),
        }
}

pub async fn get_route(
    tables: &mut Tables,
    face: &Arc<FaceState>,
    rid: ZInt,
    suffix: &str,
    info: &Option<DataInfo>,
    payload: &RBuf,
) -> Option<DataRoute> {
    match tables.get_mapping(&face, &rid) {
        Some(prefix) => unsafe {
            match Resource::get_resource(prefix, suffix) {
                Some(res) => {
                    for mres in &res.matches {
                        let mut mres = mres.upgrade().unwrap();
                        let mres = Arc::get_mut_unchecked(&mut mres);
                        for mut context in mres.contexts.values_mut() {
                            if let Some(subinfo) = &context.subs {
                                if SubMode::Pull == subinfo.mode {
                                    Arc::get_mut_unchecked(&mut context).last_values.insert(
                                        [&prefix.name(), suffix].concat(),
                                        (info.clone(), payload.clone()),
                                    );
                                }
                            }
                        }
                    }

                    Some(res.route.clone())
                }
                None => {
                    let mut faces = HashMap::new();
                    let resname = [&prefix.name(), suffix].concat();
                    for mres in Resource::get_matches(&tables, &resname) {
                        let mut mres = mres.upgrade().unwrap();
                        let mres = Arc::get_mut_unchecked(&mut mres);
                        for (sid, mut context) in &mut mres.contexts {
                            if let Some(subinfo) = &context.subs {
                                match subinfo.mode {
                                    SubMode::Pull => {
                                        Arc::get_mut_unchecked(&mut context).last_values.insert(
                                            resname.clone(),
                                            (info.clone(), payload.clone()),
                                        );
                                    }
                                    SubMode::Push => {
                                        faces.entry(*sid).or_insert_with(|| {
                                            let reskey =
                                                Resource::get_best_key(prefix, suffix, *sid);
                                            (context.face.clone(), reskey)
                                        });
                                    }
                                }
                            }
                        }
                    }
                    Some(faces)
                }
            }
        },
        None => {
            log::error!("Route data with unknown rid {}!", rid);
            None
        }
    }
}

pub async fn route_data(
    tables: &mut Tables,
    face: &Arc<FaceState>,
    rid: u64,
    suffix: &str,
    congestion_control: CongestionControl,
    info: Option<DataInfo>,
    payload: RBuf,
) {
    if let Some(route) = get_route(tables, face, rid, suffix, &info, &payload).await {
        // if an HLC was configured (via Config.add_timestamp),
        // check DataInfo and add a timestamp if there isn't
        let data_info = match &tables.hlc {
            Some(hlc) => match treat_timestamp(hlc, info).await {
                Ok(info) => info,
                Err(e) => {
                    log::error!(
                        "Error treating timestamp for received Data ({}): drop it!",
                        e
                    );
                    return;
                }
            },
            None => info,
        };

        for (_id, (outface, reskey)) in route {
            if propagate_data(tables.whatami, face, &outface) {
                outface
                    .primitives
                    .data(
                        &reskey,
                        payload.clone(),
                        Reliability::Reliable, // TODO: Need to check the active subscriptions to determine the right reliability value
                        congestion_control,
                        data_info.clone(),
                        None,
                    )
                    .await
            }
        }
    }
}

async fn treat_timestamp(hlc: &HLC, info: Option<DataInfo>) -> Result<Option<DataInfo>, String> {
    if let Some(mut data_info) = info {
        if let Some(ref ts) = data_info.timestamp {
            // Timestamp is present; update HLC with it (possibly raising error if delta exceed)
            hlc.update_with_timestamp(ts).await?;
            Ok(Some(data_info))
        } else {
            // Timestamp not present; add one
            data_info.timestamp = Some(hlc.new_timestamp().await);
            log::trace!("Adding timestamp to DataInfo: {:?}", data_info.timestamp);
            Ok(Some(data_info))
        }
    } else {
        // No DataInfo; add one with a Timestamp
        Ok(Some(new_datainfo(hlc.new_timestamp().await)))
    }
}

fn new_datainfo(ts: uhlc::Timestamp) -> DataInfo {
    DataInfo {
        source_id: None,
        source_sn: None,
        first_router_id: None,
        first_router_sn: None,
        timestamp: Some(ts),
        kind: None,
        encoding: None,
    }
}

pub async fn pull_data(
    tables: &mut Tables,
    face: &Arc<FaceState>,
    _is_final: bool,
    rid: ZInt,
    suffix: &str,
    _pull_id: ZInt,
    _max_samples: &Option<ZInt>,
) {
    match tables.get_mapping(&face, &rid) {
        Some(prefix) => match Resource::get_resource(prefix, suffix) {
            Some(mut res) => unsafe {
                let res = Arc::get_mut_unchecked(&mut res);
                match res.contexts.get_mut(&face.id) {
                    Some(mut ctx) => match &ctx.subs {
                        Some(subinfo) => {
                            for (name, (info, data)) in &ctx.last_values {
                                let reskey =
                                    Resource::get_best_key(&tables.root_res, name, face.id);
                                face.primitives
                                    .data(
                                        &reskey,
                                        data.clone(),
                                        subinfo.reliability,
                                        CongestionControl::Drop, // TODO: Default value for the time being
                                        info.clone(),
                                        None,
                                    )
                                    .await;
                            }
                            Arc::get_mut_unchecked(&mut ctx).last_values.clear();
                        }
                        None => {
                            log::error!(
                                "Pull data for unknown subscription {} (no info)!",
                                [&prefix.name(), suffix].concat()
                            );
                        }
                    },
                    None => {
                        log::error!(
                            "Pull data for unknown subscription {} (no context)!",
                            [&prefix.name(), suffix].concat()
                        );
                    }
                }
            },
            None => {
                log::error!(
                    "Pull data for unknown subscription {} (no resource)!",
                    [&prefix.name(), suffix].concat()
                );
            }
        },
        None => {
            log::error!("Pull data with unknown rid {}!", rid);
        }
    };
}
