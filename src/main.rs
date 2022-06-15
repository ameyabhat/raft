use clap::Parser;
use messages::RecvOptions;
use replica::ReplicaState;
use std::io::Result;
use std::task::Poll;

mod messages;
mod replica;

#[derive(Parser)]
struct Args {
    machine_id: String,
    replica_ids: Vec<String>,
}

// What do I have left to do?
// TODO: Implement Leader Elections
//   - init replicas with randomized timeouts (Done)
//   - identify missing leaders (time since last append entry > randomized timeout) (Done)
//   - start an election - send out a RequestVote RPC to all messages and
//     transition my state to candidate (Done)
//   - vote / respond to RequestVote (I vote for anyone as long as their log
//     is at least as long as mine and I haven't voted for someone in the same term)
//     (Done)
//   - If I receive an append entry from an equal or higher term than mine, I transition from
//     candidate to follower (DONE)
//   - If I get a majority of votes, transition to state leader and start sending out append entries
//     (Done)

// TODO 6/7
// - Fix this to use poll_recv instead of an async timeout (Done) (Kinda?)
// - Implement append entry - heartbeats, and if I receive an append entry from
//   a term equal to or higher than mine (AS a candidate),  i switch back to
//   follower (Done - if the context/ poll recv bullshit wokrs)
// - Implement the Vote RPC - (Done)

// Next Milestone - Log Replication: Still need to break down what this is
/// -
///
#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    let mut m = replica::new(&args.machine_id, &args.replica_ids).await?;

    loop {
        let attempt_read = m.read().await;

        // This timeout could cause problems - it resets if we get any message,
        // not just messages from the leader. This means that illegitimate leaders
        // and clients can send us messages, and the timeout will reset. Could lock us here.

        // the leader could also miss a heartbeat b/c a message was redirected from a follower

        // We need to find a way to make sure that the timeout only resets on
        // appendEntry messages from the actual leader

        // I think I need to rewrite this to use poll_select, but I should look into it more
        match attempt_read {
            Poll::Ready(recv_msg) => {
                println!("{:?}", recv_msg);
                let body = recv_msg.body;

                match recv_msg.options {
                    RecvOptions::Put { key, value } => {
                        if m.is_leader() {
                            m.commit(key, value);
                        } else {
                            m.redirect(&body.mid).await?;
                        }
                    }
                    RecvOptions::Get { key } => {
                        if m.is_leader() {
                            m.get(&key, &body.src, &body.mid).await?;
                        } else {
                            m.redirect(&body.mid).await?;
                        }
                    }
                    RecvOptions::RequestVote {
                        term,
                        last_log_index,
                        last_log_term,
                    } => {
                        // See if we should vote
                        if !m.vote_history.contains(&term)
                            && m.as_least_as_long(last_log_index, last_log_term)
                        {
                            m.vote(&body.src, term).await?
                        }
                    }
                    RecvOptions::Vote { term } => {
                        if matches!(m.state, ReplicaState::Candidate) && term == m.term {
                            // 1. tally the vote (this key should already exist in the map because I voted for myself)
                            let num_votes_in_term = m.vote_tally.entry(term).or_insert(1);

                            let required_vote_threshold: u16 =
                                ((m.colleagues.len() / 2) + 1).try_into().unwrap();
                            // 2. see if we're the leader yet
                            if *num_votes_in_term >= required_vote_threshold {
                                // 3. Change our status to leader
                                m.state = ReplicaState::Leader;

                                // 4. Send an append entry
                                m.send_heartbeat(term).await?;
                            }
                        }
                    }
                    RecvOptions::AppendEntry {
                        term,
                        leader_id,
                        prev_log_index,
                        prev_log_term,
                        entries,
                        leader_commit_index,
                    } => {
                        m.reset_time_of_last_heartbeat();
                        m.should_accept_leader(term, leader_id)
                    }
                    RecvOptions::AppendEntryResult { term, success } => {}
                }
            }
            // We haven't received a message in {election_timeout} milliseconds
            // Right now this tells us that we haven't received any message -
            // it should tell us if we haven't received a message from the leader
            // nbd - just reset the timeout only on messages from the leader
            Poll::Pending => {
                if m.election_timeout_elapsed() {
                    if m.is_leader() {
                        // send heartbeat
                        m.send_heartbeat(m.term).await?;
                    } else {
                        if let Err(x) = m.start_election().await {
                            panic!("{:?}: Unrecoverable failure starting elections", x)
                        }
                    }
                }
            }
        }
    }
}
