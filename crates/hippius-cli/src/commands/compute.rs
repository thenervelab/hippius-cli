//! `hipc compute`: the VM catalog, and one module per compute resource.
//!
//! The compute routes are account-scoped, so a membership sends `X-Hippius-Account` (tenet
//! 12). `/api/ssh-keys/` is personal: the header is never sent there, and a membership still
//! sends the request. The owner's keys, `/api/compute/account-ssh-keys/`, are under compute
//! and scoped like the rest of it. `hipc` opens no SSH session. A launch exposes no port at
//! the edge (`exposures create` does, as its own confirmed command), though `--open-port`
//! sets the new VM's firewall rules. A launch attaches a public IPv4 only with
//! `--public-ip`; `public-ip create` attaches one to a VM that exists.
//!
//! Managed databases are in [`databases`]: the console and the grants group them with
//! compute, though their routes are under `/api/databases/`.
//!
//! Placement groups are read here and named at launch. There is no route to create or
//! delete one: a group exists while a live VM names it.
//!
//! A VM launch is in [`vms`], and what it shares with an App Engine launch ([`app_engine`]),
//! the wait included, in [`launch`].

mod app_engine;

mod backups;

mod databases;

mod exposures;

mod firewall;

mod kubernetes;

mod launch;

mod power_policy;

mod public_ip;

pub(crate) mod runners;

mod ssh_keys;

mod vms;

pub(crate) use backups::BACKUPS_KEPT;
pub(crate) use exposures::EXPOSURE_DELETED;
pub(crate) use public_ip::IP_DETACHED;
pub(crate) use ssh_keys::SSH_KEY_KEPT;
pub(crate) use vms::VM_LEFT_BILLED;

use hippius::routes::Route;
use hippius::routes::compute;
use hippius::{Error, Request, Result, view_of};

use crate::cli::{Compute, Firewall, VmTarget};
use crate::commands::{Printed, print_body, print_envelope};
use crate::context::{self, Context, Gate};

pub(crate) fn run(ctx: &Context<'_>, command: &Compute) -> Result<Printed> {
    let get = |route: &Route<0>, action: &str| {
        let client = ctx.load_authed_client()?;
        let request = Request::to(route, []).about(action);
        print_body(ctx, &client, &request, Gate::Open)
    };
    match command {
        Compute::Prices => get(&compute::PRICES, "get the compute prices"),
        // Swagger documents the public list with no security, so it sends a token only when
        // one is configured, as `cdn prices` does. It is a command of its own, not what
        // `prices` reads when no token is set: the two answers differ in shape and unit,
        // and a document must not change with the configuration (tenet 15).
        Compute::PublicPrices => print_body(
            ctx,
            &ctx.client(&context::config()?)?,
            &Request::to(&compute::PUBLIC_PRICES, []).about("get the public compute prices"),
            Gate::Open,
        ),
        Compute::Quota => get(&compute::QUOTA, "get the compute quota"),
        Compute::ChargeCap => get(&compute::CHARGE_CAP, "get the compute charge cap"),
        Compute::Images => get(&compute::IMAGES, "list the VM images"),
        Compute::PlacementGroups => get(&compute::PLACEMENT_GROUPS, "list the placement groups"),
        Compute::Flavors => print_envelope(
            ctx,
            &ctx.load_authed_client()?,
            &Request::to(&compute::FLAVORS, []).about("list the VM flavors"),
        ),
        Compute::Regions => print_envelope(
            ctx,
            &ctx.load_authed_client()?,
            &Request::to(&compute::REGIONS, []).about("list the VM regions"),
        ),
        // The console reads the apps catalog and the owner's keys as bare arrays;
        // swagger documents no body.
        Compute::Apps => print_envelope(
            ctx,
            &ctx.load_authed_client()?,
            &Request::to(&compute::APPS, []).about("list the ready-made apps"),
        ),
        Compute::AppEngine(command) => app_engine::run(ctx, command),
        Compute::AccountSshKeys => print_envelope(
            ctx,
            &ctx.load_authed_client()?,
            &Request::to(&compute::ACCOUNT_SSH_KEYS, [])
                .about("list the SSH keys of the account's owner"),
        ),
        Compute::Feasibility {
            flavor,
            region,
            os_disk_tier,
        } => {
            let request = Request::to(&compute::FEASIBILITY, [])
                .query("flavor", flavor)
                .about(format!("check whether the flavor `{flavor}` can launch"))
                .listed_by("hipc compute flavors");
            let request = match region {
                Some(region) => request.query("region", region),
                None => request,
            };
            let request = match os_disk_tier {
                Some(tier) => request.query("os_disk_tier", tier.code()),
                None => request,
            };
            print_body(ctx, &ctx.load_authed_client()?, &request, Gate::Open)
        }
        Compute::Vms(command) => vms::vms(ctx, &ctx.load_authed_client()?, command),
        Compute::PublicIp(command) => {
            public_ip::public_ip(ctx, &ctx.load_authed_client()?, command)
        }
        Compute::Firewall(Firewall::Get(VmTarget { vm_id })) => print_body(
            ctx,
            &ctx.load_authed_client()?,
            &firewall::firewall_get(vm_id.as_str()),
            Gate::Open,
        ),
        Compute::Firewall(Firewall::Update(args)) => firewall::update(ctx, args),
        Compute::Exposures(command) => exposures::exposures(ctx, command),
        Compute::Backups(command) => backups::backups(ctx, &ctx.load_authed_client()?, command),
        Compute::Runners(command) => runners::run(ctx, command),
        Compute::SshKeys(command) => ssh_keys::ssh_keys(ctx, command),
        Compute::Databases(command) => databases::run(ctx, command),
        Compute::Kubernetes(command) => kubernetes::run(ctx, command),
    }
}

pub(super) const VMS_LIST_COMMAND: &str = "hipc compute vms list";

/// A request to `route` of the VM `vm_id`, whose action is `verb` and the VM, such as
/// `get the backups of` and then "the VM `vm_1`"; a VM that is not there is found with
/// [`VMS_LIST_COMMAND`].
fn of_vm(route: &Route<1>, verb: &str, vm_id: &dyn std::fmt::Display) -> Request {
    Request::to(route, [vm_id])
        .about(format!("{verb} the VM `{vm_id}`"))
        .listed_by(VMS_LIST_COMMAND)
}

/// Whether the server answered `status` with the compute error slug `slug`
/// ([`compute::Refusal`]).
pub(super) fn refused_as(error: &Error, status: u16, slug: &str) -> bool {
    error.http_status() == Some(status)
        && error
            .details()
            .is_some_and(|body| view_of::<compute::Refusal>(body).is(slug))
}
