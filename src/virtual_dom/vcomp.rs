//! This module contains the implementation of a virtual component `VComp`.

use super::{VDiff, VNode};
use crate::callback::Callback;
use crate::html::{Component, ComponentUpdate, NodeRef, Scope};
use std::any::TypeId;
use std::cell::RefCell;
use std::fmt;
use std::rc::Rc;
use stdweb::web::{document, Element, INode, Node, TextNode};

struct Hidden;

type HiddenScope = *mut Hidden;

/// The method generates an instance of a component.
type Generator<PARENT> = dyn FnOnce(GeneratorType, Scope<PARENT>) -> Mounted;

/// Components can be generated by mounting or by overwriting an old component.
enum GeneratorType {
    Mount(Element, TextNode),
    Overwrite(TypeId, HiddenScope),
}

/// A reference to the parent's scope which will be used later to send messages.
pub type ScopeHolder<PARENT> = Rc<RefCell<Option<Scope<PARENT>>>>;

/// A virtual component.
pub struct VComp<PARENT: Component> {
    type_id: TypeId,
    state: Rc<RefCell<MountState<PARENT>>>,
}

/// A virtual child component.
pub struct VChild<SELF: Component, PARENT: Component> {
    /// The component properties
    pub props: SELF::Properties,
    /// The parent component scope
    pub scope: ScopeHolder<PARENT>,
    /// Reference to the mounted node
    node_ref: NodeRef,
}

impl<SELF, PARENT> VChild<SELF, PARENT>
where
    SELF: Component,
    PARENT: Component,
{
    /// Creates a child component that can be accessed and modified by its parent.
    pub fn new(props: SELF::Properties, scope: ScopeHolder<PARENT>, node_ref: NodeRef) -> Self {
        Self {
            props,
            scope,
            node_ref,
        }
    }
}

impl<SELF, PARENT> From<VChild<SELF, PARENT>> for VComp<PARENT>
where
    SELF: Component,
    PARENT: Component,
{
    fn from(vchild: VChild<SELF, PARENT>) -> Self {
        VComp::new::<SELF>(vchild.props, vchild.scope, vchild.node_ref)
    }
}

enum MountState<PARENT: Component> {
    Unmounted(Unmounted<PARENT>),
    Mounted(Mounted),
    Mounting,
    Detached,
    Overwritten,
}

struct Unmounted<PARENT: Component> {
    generator: Box<Generator<PARENT>>,
}

struct Mounted {
    node_ref: NodeRef,
    scope: HiddenScope,
    destroyer: Box<dyn FnOnce()>,
}

impl<PARENT: Component> VComp<PARENT> {
    /// This method prepares a generator to make a new instance of the `Component`.
    pub fn new<SELF>(
        props: SELF::Properties,
        scope_holder: ScopeHolder<PARENT>,
        node_ref: NodeRef,
    ) -> Self
    where
        SELF: Component,
    {
        let generator = move |generator_type: GeneratorType, parent: Scope<PARENT>| -> Mounted {
            *scope_holder.borrow_mut() = Some(parent);
            match generator_type {
                GeneratorType::Mount(element, dummy_node) => {
                    let scope: Scope<SELF> = Scope::new();

                    let mut scope = scope.mount_in_place(
                        element,
                        Some(VNode::VRef(dummy_node.into())),
                        node_ref.clone(),
                        props,
                    );

                    Mounted {
                        node_ref,
                        scope: Box::into_raw(Box::new(scope.clone())) as *mut Hidden,
                        destroyer: Box::new(move || scope.destroy()),
                    }
                }
                GeneratorType::Overwrite(type_id, scope) => {
                    if type_id != TypeId::of::<SELF>() {
                        panic!("tried to overwrite a different type of component");
                    }

                    let mut scope = unsafe {
                        let raw: *mut Scope<SELF> = scope as *mut Scope<SELF>;
                        *Box::from_raw(raw)
                    };

                    scope.update(ComponentUpdate::Properties(props));

                    Mounted {
                        node_ref,
                        scope: Box::into_raw(Box::new(scope.clone())) as *mut Hidden,
                        destroyer: Box::new(move || scope.destroy()),
                    }
                }
            }
        };

        VComp {
            type_id: TypeId::of::<SELF>(),
            state: Rc::new(RefCell::new(MountState::Unmounted(Unmounted {
                generator: Box::new(generator),
            }))),
        }
    }
}

/// Transforms properties and attaches a parent scope holder to callbacks for sending messages.
pub trait Transformer<PARENT: Component, FROM, TO> {
    /// Transforms one type to another.
    fn transform(scope_holder: ScopeHolder<PARENT>, from: FROM) -> TO;
}

impl<PARENT, T> Transformer<PARENT, T, T> for VComp<PARENT>
where
    PARENT: Component,
{
    fn transform(_: ScopeHolder<PARENT>, from: T) -> T {
        from
    }
}

impl<'a, PARENT, T> Transformer<PARENT, &'a T, T> for VComp<PARENT>
where
    PARENT: Component,
    T: Clone,
{
    fn transform(_: ScopeHolder<PARENT>, from: &'a T) -> T {
        from.clone()
    }
}

impl<'a, PARENT> Transformer<PARENT, &'a str, String> for VComp<PARENT>
where
    PARENT: Component,
{
    fn transform(_: ScopeHolder<PARENT>, from: &'a str) -> String {
        from.to_owned()
    }
}

impl<'a, PARENT, F, IN> Transformer<PARENT, F, Callback<IN>> for VComp<PARENT>
where
    PARENT: Component,
    F: Fn(IN) -> PARENT::Message + 'static,
{
    fn transform(scope: ScopeHolder<PARENT>, from: F) -> Callback<IN> {
        let callback = move |arg| {
            let msg = from(arg);
            if let Some(ref mut sender) = *scope.borrow_mut() {
                sender.send_message(msg);
            } else {
                panic!("Parent component hasn't activated this callback yet");
            }
        };
        callback.into()
    }
}

impl<'a, PARENT, F, IN> Transformer<PARENT, F, Option<Callback<IN>>> for VComp<PARENT>
where
    PARENT: Component,
    F: Fn(IN) -> PARENT::Message + 'static,
{
    fn transform(scope: ScopeHolder<PARENT>, from: F) -> Option<Callback<IN>> {
        let callback = move |arg| {
            let msg = from(arg);
            if let Some(ref mut sender) = *scope.borrow_mut() {
                sender.send_message(msg);
            } else {
                panic!("Parent component hasn't activated this callback yet");
            }
        };
        Some(callback.into())
    }
}

impl<PARENT: Component> Unmounted<PARENT> {
    /// Mount a virtual component using a generator.
    fn mount(self, parent: Element, dummy_node: TextNode, parent_scope: Scope<PARENT>) -> Mounted {
        (self.generator)(GeneratorType::Mount(parent, dummy_node), parent_scope)
    }

    /// Overwrite an existing virtual component using a generator.
    fn replace(self, type_id: TypeId, old: Mounted, parent_scope: Scope<PARENT>) -> Mounted {
        (self.generator)(GeneratorType::Overwrite(type_id, old.scope), parent_scope)
    }
}

enum Reform {
    Keep(TypeId, Mounted),
    Before(Option<Node>),
}

impl<COMP> VDiff for VComp<COMP>
where
    COMP: Component + 'static,
{
    type Component = COMP;

    fn detach(&mut self, parent: &Element) -> Option<Node> {
        match self.state.replace(MountState::Detached) {
            MountState::Mounted(this) => {
                (this.destroyer)();
                this.node_ref.get().and_then(|node| {
                    let sibling = node.next_sibling();
                    parent
                        .remove_child(&node)
                        .expect("can't remove the component");
                    sibling
                })
            }
            _ => None,
        }
    }

    fn apply(
        &mut self,
        parent: &Element,
        previous_sibling: Option<&Node>,
        ancestor: Option<VNode<Self::Component>>,
        parent_scope: &Scope<Self::Component>,
    ) -> Option<Node> {
        match self.state.replace(MountState::Mounting) {
            MountState::Unmounted(this) => {
                let reform = match ancestor {
                    Some(VNode::VComp(mut vcomp)) => {
                        // If the ancestor is a Component of the same type, don't replace, keep the
                        // old Component but update the properties.
                        if self.type_id == vcomp.type_id {
                            match vcomp.state.replace(MountState::Overwritten) {
                                MountState::Mounted(mounted) => {
                                    Reform::Keep(vcomp.type_id, mounted)
                                }
                                _ => Reform::Before(None),
                            }
                        } else {
                            let node = vcomp.detach(parent);
                            Reform::Before(node)
                        }
                    }
                    Some(mut vnode) => {
                        let node = vnode.detach(parent);
                        Reform::Before(node)
                    }
                    None => Reform::Before(None),
                };

                let mounted = match reform {
                    Reform::Keep(type_id, mounted) => {
                        // Send properties update when the component is already rendered.
                        this.replace(type_id, mounted, parent_scope.clone())
                    }
                    Reform::Before(before) => {
                        // Temporary node which will be replaced by a component's root node.
                        let dummy_node = document().create_text_node("");
                        if let Some(sibling) = before {
                            parent
                                .insert_before(&dummy_node, &sibling)
                                .expect("can't insert dummy node for a component");
                        } else {
                            let previous_sibling =
                                previous_sibling.and_then(|before| before.next_sibling());
                            if let Some(previous_sibling) = previous_sibling {
                                parent
                                    .insert_before(&dummy_node, &previous_sibling)
                                    .expect("can't insert dummy node before previous sibling");
                            } else {
                                parent.append_child(&dummy_node);
                            }
                        }
                        this.mount(parent.to_owned(), dummy_node, parent_scope.clone())
                    }
                };

                let node = mounted.node_ref.get();
                self.state.replace(MountState::Mounted(mounted));
                node
            }
            state => {
                self.state.replace(state);
                None
            }
        }
    }
}

impl<C: Component> PartialEq for VComp<C> {
    fn eq(&self, other: &VComp<C>) -> bool {
        self.type_id == other.type_id
    }
}

impl<C: Component> fmt::Debug for VComp<C> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("VComp<_>")
    }
}

impl<SELF: Component, PARENT: Component> fmt::Debug for VChild<SELF, PARENT> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("VChild<_,_>")
    }
}
