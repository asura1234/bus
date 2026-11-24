use agile_board::{
    error::{invalid, Error, Result},
    mock,
    model::{now_epoch_ms, Column, NewTicketInput, UpdateTicketInput, ViewFilter},
    server, store,
};
use clap::{Args, Parser, Subcommand};
use serde_json::{json, Value};
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

#[derive(Parser)]
#[command(
    name = "agile-board",
    version,
    about = "Read-only localhost web Kanban board driven by a CLI"
)]
struct Cli {
    /// Board data directory (must be an absolute path)
    #[arg(long, global = true)]
    data_dir: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Serve the read-only board on 127.0.0.1 and print the URL
    Serve(ServeArgs),
    /// Register or list board sections
    Room {
        #[command(subcommand)]
        action: RoomAction,
    },
    /// Create, move, update, or list tickets
    Ticket {
        #[command(subcommand)]
        action: TicketAction,
    },
    /// Inspect or clear session activity history
    History(HistoryArgs),
}

#[derive(Args)]
struct ServeArgs {
    /// TCP port on 127.0.0.1 (0 picks an ephemeral free port; default 8787)
    #[arg(long, default_value_t = 8787)]
    port: u16,
    /// Seed realistic mock sections and tickets if the board is empty
    #[arg(long)]
    mock: bool,
    /// Optional wall-clock bound in seconds for testing/automation
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
    run_for: Option<u64>,
}

#[derive(Subcommand)]
enum RoomAction {
    /// Register or update a board section
    Add {
        #[arg(long)]
        id: u64,
        #[arg(long)]
        name: String,
        #[arg(long)]
        json: bool,
    },
    /// List registered board sections
    List {
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum TicketAction {
    /// Create a new ticket in a registered section
    Add {
        /// Section name or numeric ID
        #[arg(long)]
        room: String,
        /// Ticket plain-text title
        #[arg(long, allow_hyphen_values = true)]
        title: String,
        /// One-sentence Markdown description (links rendered safely)
        #[arg(long, allow_hyphen_values = true)]
        description: Option<String>,
        /// Initial column
        #[arg(long, value_enum, default_value_t = Column::Todo)]
        column: Column,
        /// Separate MR/PR URL
        #[arg(long)]
        mr: Option<String>,
        /// Allow this ticket to transition directly from investigation -> done
        #[arg(long)]
        investigation_only: bool,
        /// Optional note recorded in ticket history
        #[arg(long, allow_hyphen_values = true)]
        note: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Move a ticket to a new column with validated transitions
    Move {
        /// Ticket short ID (e.g. T-1)
        id: String,
        /// Target column
        #[arg(long, value_enum)]
        column: Column,
        /// Transition note recorded on the ticket and in session history
        #[arg(long, allow_hyphen_values = true)]
        note: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Update a ticket's title, Markdown description, MR URL, or append a note
    Update {
        /// Ticket short ID (e.g. T-1)
        id: String,
        #[arg(long, allow_hyphen_values = true)]
        title: Option<String>,
        #[arg(long, allow_hyphen_values = true)]
        description: Option<String>,
        #[arg(long)]
        mr: Option<String>,
        #[arg(long, allow_hyphen_values = true)]
        note: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// List tickets filtered by section, column, or view window (day|week|session)
    List {
        #[arg(long)]
        room: Option<String>,
        #[arg(long, value_enum)]
        column: Option<Column>,
        #[arg(long, value_enum, default_value_t = ViewFilter::Session)]
        view: ViewFilter,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Args)]
struct HistoryArgs {
    #[command(subcommand)]
    sub: Option<HistorySubcommand>,
    #[arg(long)]
    json: bool,
}

#[derive(Subcommand)]
enum HistorySubcommand {
    /// Clear all tickets and session history for this board session (retains registered sections)
    Clear {
        /// Explicit confirmation required to clear session tickets and history
        #[arg(long)]
        confirm: bool,
        #[arg(long)]
        json: bool,
    },
}

fn execute(cli: Cli) -> Result<Option<String>> {
    let data_dir = store::resolve_data_dir(cli.data_dir)?;
    let now_ms = now_epoch_ms();

    match cli.command {
        Command::Serve(args) => {
            if args.mock {
                store::mutate(&data_dir, |s| {
                    if s.tickets.is_empty() && s.rooms.is_empty() {
                        mock::seed_mock_state(s, now_ms)?;
                    }
                    Ok(())
                })?;
            }
            let stop = Arc::new(AtomicBool::new(false));
            let s_flag = stop.clone();
            ctrlc::set_handler(move || s_flag.store(true, Ordering::SeqCst))
                .map_err(|_| Error(1, "cannot install signal handler".into()))?;
            server::serve(&data_dir, args.port, args.run_for, stop)?;
            Ok(None)
        }
        Command::Room { action } => match action {
            RoomAction::Add {
                id,
                name,
                json: as_json,
            } => {
                let (room, _) = store::mutate(&data_dir, |s| s.add_room(id, name, now_ms))?;
                if as_json {
                    Ok(Some(json!({"ok": true, "room": room}).to_string()))
                } else {
                    Ok(Some(format!(
                        "Registered section #{}: {}",
                        room.id, room.name
                    )))
                }
            }
            RoomAction::List { json: as_json } => {
                let state = store::load(&data_dir)?;
                if as_json {
                    Ok(Some(json!({"ok": true, "rooms": state.rooms}).to_string()))
                } else if state.rooms.is_empty() {
                    Ok(Some("No sections registered.".into()))
                } else {
                    let lines = state
                        .rooms
                        .iter()
                        .map(|r| format!("#{}\t{}", r.id, r.name))
                        .collect::<Vec<_>>()
                        .join("\n");
                    Ok(Some(lines))
                }
            }
        },
        Command::Ticket { action } => match action {
            TicketAction::Add {
                room,
                title,
                description,
                column,
                mr,
                investigation_only,
                note,
                json: as_json,
            } => {
                let (ticket, _) = store::mutate(&data_dir, |s| {
                    s.add_ticket(
                        NewTicketInput {
                            room,
                            title,
                            description,
                            column,
                            mr_url: mr,
                            investigation_only,
                            note,
                        },
                        now_ms,
                    )
                })?;
                if as_json {
                    Ok(Some(json!({"ok": true, "ticket": ticket}).to_string()))
                } else {
                    Ok(Some(format!(
                        "{}\t[{}]\t#{}:{}\t{}",
                        ticket.id,
                        ticket.column.as_str(),
                        ticket.room_id,
                        ticket.room_name,
                        ticket.title
                    )))
                }
            }
            TicketAction::Move {
                id,
                column,
                note,
                json: as_json,
            } => {
                let (ticket, _) =
                    store::mutate(&data_dir, |s| s.move_ticket(&id, column, note, now_ms))?;
                if as_json {
                    Ok(Some(json!({"ok": true, "ticket": ticket}).to_string()))
                } else {
                    Ok(Some(format!(
                        "Moved {} -> {} ({})",
                        ticket.id,
                        ticket.column.as_str(),
                        ticket.title
                    )))
                }
            }
            TicketAction::Update {
                id,
                title,
                description,
                mr,
                note,
                json: as_json,
            } => {
                let (ticket, _) = store::mutate(&data_dir, |s| {
                    s.update_ticket(
                        UpdateTicketInput {
                            id,
                            title,
                            description,
                            mr_url: mr,
                            note,
                        },
                        now_ms,
                    )
                })?;
                if as_json {
                    Ok(Some(json!({"ok": true, "ticket": ticket}).to_string()))
                } else {
                    Ok(Some(format!(
                        "Updated {} [{}] {}",
                        ticket.id,
                        ticket.column.as_str(),
                        ticket.title
                    )))
                }
            }
            TicketAction::List {
                room,
                column,
                view,
                json: as_json,
            } => {
                let state = store::load(&data_dir)?;
                let room_filter_id = match room {
                    Some(ref r) => Some(state.resolve_room(r)?.id),
                    None => None,
                };
                let tickets = state
                    .tickets
                    .iter()
                    .filter(|t| {
                        room_filter_id.is_none_or(|rid| t.room_id == rid)
                            && column.is_none_or(|c| t.column == c)
                            && view.matches(t, now_ms)
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                if as_json {
                    Ok(Some(
                        json!({
                            "ok": true,
                            "view": view.as_str(),
                            "count": tickets.len(),
                            "tickets": tickets
                        })
                        .to_string(),
                    ))
                } else if tickets.is_empty() {
                    Ok(Some("No matching tickets.".into()))
                } else {
                    let lines = tickets
                        .iter()
                        .map(|t| {
                            let inv = if t.investigation_only {
                                " [inv-only]"
                            } else {
                                ""
                            };
                            let mr = t.mr_url.as_deref().unwrap_or("-");
                            format!(
                                "{}\t{}\t#{}:{}\t{}{}\t{}",
                                t.id,
                                t.column.as_str(),
                                t.room_id,
                                t.room_name,
                                t.title,
                                inv,
                                mr
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    Ok(Some(lines))
                }
            }
        },
        Command::History(args) => match args.sub {
            Some(HistorySubcommand::Clear {
                confirm,
                json: as_json,
            }) => {
                if !confirm {
                    return Err(invalid(
                        "refusing to clear session history without --confirm (clears all tickets and history in this session)",
                    ));
                }
                let ((cleared_tickets, cleared_events), _) =
                    store::mutate(&data_dir, |s| Ok(s.clear_session_history()))?;
                if as_json || args.json {
                    Ok(Some(
                        json!({
                            "ok": true,
                            "cleared_tickets": cleared_tickets,
                            "cleared_events": cleared_events
                        })
                        .to_string(),
                    ))
                } else {
                    Ok(Some(format!(
                        "Cleared session board: {cleared_tickets} tickets and {cleared_events} history events removed."
                    )))
                }
            }
            None => {
                let state = store::load(&data_dir)?;
                if args.json {
                    let out: Value = json!({
                        "ok": true,
                        "count": state.session_history.len(),
                        "history": state.session_history
                    });
                    Ok(Some(out.to_string()))
                } else if state.session_history.is_empty() {
                    Ok(Some("No session history events.".into()))
                } else {
                    let lines = state
                        .session_history
                        .iter()
                        .map(|ev| {
                            let note_suffix = ev
                                .note
                                .as_deref()
                                .map(|n| format!(" — {n}"))
                                .unwrap_or_default();
                            format!(
                                "#{}\t{}\t{}\t{}{}",
                                ev.seq, ev.at_ms, ev.kind, ev.summary, note_suffix
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    Ok(Some(lines))
                }
            }
        },
    }
}

fn main() {
    match execute(Cli::parse()) {
        Ok(Some(out)) => println!("{out}"),
        Ok(None) => {}
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(e.0);
        }
    }
}
