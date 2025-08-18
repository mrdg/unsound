use std::collections::HashSet;

use slotmap::{new_key_type, SlotMap};

pub const DEFAULT_SIZE: usize = 128;

new_key_type! { pub struct NodeId; }

#[derive(Clone)]
enum Node {
    Buffer(HashSet<NodeId>),
    Effect(Vec<Option<NodeId>>),
    Instrument,
}

impl Node {
    fn buffer() -> Self {
        Self::Buffer(HashSet::new())
    }

    fn effect(inputs: usize) -> Self {
        Self::Effect(vec![None; inputs])
    }

    fn inputs(&self) -> Vec<&NodeId> {
        match self {
            Self::Buffer(inputs) => {
                let mut vec: Vec<&NodeId> = inputs.iter().collect();
                vec.sort();
                vec
            }
            Self::Effect(inputs) => inputs.iter().flatten().collect(),
            _ => Vec::new(),
        }
    }

    fn remove_input(&mut self, node_id: NodeId) {
        match self {
            Self::Buffer(inputs) => {
                inputs.remove(&node_id);
            }
            Self::Effect(inputs) => {
                if let Some(idx) = inputs.iter().position(|i| *i == Some(node_id)) {
                    inputs[idx] = None;
                }
            }
            _ => {}
        }
    }
}

#[derive(Debug, Clone)]
pub struct TrackNode {
    pub output: NodeId,
    pub buffer: NodeId,
}

pub struct AudioGraph {
    pub main_output: NodeId,
    pub tmp_buffer1: NodeId,
    pub tmp_buffer2: NodeId,

    nodes: SlotMap<NodeId, Node>,

    // Tracks for which nodes a delete command has been sent to the engine, to
    // avoid sending more than one for the same node id.
    deleted: HashSet<NodeId>,
}

impl AudioGraph {
    pub fn new() -> Self {
        let mut nodes = SlotMap::with_capacity_and_key(DEFAULT_SIZE);

        let main_output = nodes.insert(Node::buffer());
        let tmp_buffer1 = nodes.insert(Node::buffer());
        let tmp_buffer2 = nodes.insert(Node::buffer());

        Self {
            nodes,
            main_output,
            tmp_buffer1,
            tmp_buffer2,
            deleted: HashSet::new(),
        }
    }

    pub fn sort(&self) -> Schedule {
        let mut visited = HashSet::new();
        let mut entries = Vec::new();

        self.dfs(self.main_output, &mut visited, &mut entries, None);

        for (id, node) in self.nodes.iter() {
            if let Node::Instrument = node {
                assert!(!visited.contains(&id));
                entries.push(ScheduleEntry::new(id, None, None));
            }
        }

        // The graph is traversed from the main output back to the inputs so reverse
        // to get the right order: instruments first, then the tracks with their effects
        entries.reverse();

        // Ensure that schedule entries have input and output buffers. The input buffer for the
        // first effect on a track, and the output buffer for the last effect have already been
        // assigned. For effects processing within a single track we can just switch between
        // the two temp buffers.
        let mut input = self.tmp_buffer1;
        let mut output = self.tmp_buffer2;
        for entry in &mut entries {
            // Instruments don't need an output buffer. They can write into any track input buffer
            // depending on which pattern track holds the note.
            if let Node::Instrument = &self.nodes[entry.node_id] {
                continue;
            }
            if entry.input_buffer.is_none() {
                entry.input_buffer = Some(input);
            }
            if entry.output_buffer.is_none() {
                entry.output_buffer = Some(output);
            }
            (input, output) = (output, input)
        }

        Schedule {
            entries,
            main_output_buffer: Some(self.main_output),
        }
    }

    pub fn remove_node(&mut self, node_id: NodeId) {
        self.deleted.remove(&node_id);
        self.nodes.remove(node_id);
        for (_, node) in &mut self.nodes {
            node.remove_input(node_id);
        }
    }

    pub fn add_effect(&mut self) -> NodeId {
        self.add_node(Node::effect(1))
    }

    pub fn add_instrument(&mut self) -> NodeId {
        self.add_node(Node::Instrument)
    }

    pub fn add_buffer(&mut self) -> NodeId {
        self.add_node(Node::buffer())
    }

    pub fn add_track(&mut self) -> TrackNode {
        let output_node_id = self.add_node(Node::effect(1));
        let buffer_id = self.add_buffer();
        TrackNode {
            output: output_node_id,
            buffer: buffer_id,
        }
    }

    fn add_node(&mut self, node: Node) -> NodeId {
        // TODO: implement resize on the engine side
        assert!(self.nodes.capacity() - self.nodes.len() > 0);
        self.nodes.insert(node)
    }

    pub fn connect(&mut self, from: NodeId, to: NodeId) {
        match &mut self.nodes[to] {
            Node::Buffer(inputs) => {
                inputs.insert(from);
            }
            Node::Effect(inputs) => {
                inputs[0] = Some(from);
            }
            Node::Instrument => unreachable!(),
        }
    }

    pub fn orphaned_nodes(&self) -> Vec<NodeId> {
        let mut visited = HashSet::new();
        let mut entries = Vec::new();
        self.dfs(self.main_output, &mut visited, &mut entries, None);
        let mut result = Vec::new();

        for (node_id, node) in self.nodes.iter() {
            if node_id == self.tmp_buffer1 || node_id == self.tmp_buffer2 {
                continue;
            }
            match node {
                Node::Instrument => continue,
                Node::Buffer(_) | Node::Effect(_) => {
                    if visited.contains(&node_id) {
                        continue;
                    }
                }
            };
            result.push(node_id)
        }
        result
    }

    pub fn mark_deleted(&mut self, node_id: NodeId) -> bool {
        self.deleted.insert(node_id)
    }

    fn dfs(
        &self,
        node_id: NodeId,
        visited: &mut HashSet<NodeId>,
        entries: &mut Vec<ScheduleEntry>,
        output: Option<NodeId>,
    ) {
        visited.insert(node_id);

        let node = &self.nodes[node_id];
        let buffer = match node {
            Node::Buffer(_) => Some(node_id),
            _ => {
                // Only non-buffer nodes become part of the process schedule. We detect those below
                // and wire up the buffer to the last node in the entry list.
                entries.push(ScheduleEntry::new(node_id, None, output));
                None
            }
        };

        for input in node.inputs() {
            if !visited.contains(input) {
                if let Node::Buffer(_) = &self.nodes[*input] {
                    let entry = entries.last_mut().unwrap();
                    entry.input_buffer = Some(*input);
                }
                self.dfs(*input, visited, entries, buffer);
            }
        }
    }
}

#[derive(Clone, Default)]
pub struct Schedule {
    pub entries: Vec<ScheduleEntry>,
    pub main_output_buffer: Option<NodeId>,
}

#[derive(Debug, PartialEq, Clone)]
pub struct ScheduleEntry {
    pub node_id: NodeId,
    pub input_buffer: Option<NodeId>,
    pub output_buffer: Option<NodeId>,
}

impl ScheduleEntry {
    fn new(node_id: NodeId, input_buffer: Option<NodeId>, output_buffer: Option<NodeId>) -> Self {
        Self {
            node_id,
            input_buffer,
            output_buffer,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_graph_sort() {
        let mut g = AudioGraph::new();
        let master = g.add_track();
        g.connect(master.buffer, master.output);
        g.connect(master.output, g.main_output);

        let instr0 = g.add_instrument();

        let track0 = g.add_track();
        let reverb = g.add_effect();

        g.connect(track0.buffer, reverb);
        g.connect(reverb, track0.output);
        g.connect(track0.output, master.buffer);

        let track1 = g.add_track();
        let compressor = g.add_effect();

        g.connect(track1.buffer, compressor);
        g.connect(compressor, track1.output);
        g.connect(track1.output, master.buffer);

        let schedule = g.sort();
        let expected = vec![
            ScheduleEntry::new(instr0, None, None),
            // track1
            ScheduleEntry::new(compressor, Some(track1.buffer), Some(g.tmp_buffer2)),
            ScheduleEntry::new(track1.output, Some(g.tmp_buffer2), Some(master.buffer)),
            // track0
            ScheduleEntry::new(reverb, Some(track0.buffer), Some(g.tmp_buffer2)),
            ScheduleEntry::new(track0.output, Some(g.tmp_buffer2), Some(master.buffer)),
            // master
            ScheduleEntry::new(master.output, Some(master.buffer), Some(g.main_output)),
        ];
        assert_eq!(expected, schedule.entries);
    }

    #[test]
    fn test_orphaned_nodes() {
        let mut g = AudioGraph::new();
        let master = g.add_track();

        g.connect(master.buffer, master.output);
        g.connect(master.output, g.main_output);

        let _ = g.add_instrument();

        let track0 = g.add_track();
        let reverb = g.add_effect();

        g.connect(track0.buffer, reverb);
        g.connect(reverb, track0.output);
        g.connect(track0.output, master.buffer);

        g.remove_node(track0.output);

        let node_ids = g.orphaned_nodes();
        assert_eq!(vec![track0.buffer, reverb], node_ids);
    }
}
