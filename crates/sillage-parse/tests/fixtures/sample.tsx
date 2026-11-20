import React, { useState, useEffect } from "react";

interface Props {
  title: string;
  items: string[];
  onSelect?: (item: string) => void;
}

export function List({ title, items, onSelect }: Props) {
  const [selected, setSelected] = useState<string | null>(null);
  useEffect(() => {
    if (selected && onSelect) onSelect(selected);
  }, [selected, onSelect]);
  return (
    <div className="list">
      <h1>{title}</h1>
      {items.map((item) => (
        <button key={item} onClick={() => setSelected(item)} disabled={item === selected}>
          {item.toUpperCase()}
        </button>
      ))}
      <Footer count={items.length} />
    </div>
  );
}

const Footer = ({ count }: { count: number }) => <p>{count} items</p>;

export default class Counter extends React.Component<{}, { n: number }> {
  state = { n: 0 };
  increment = () => this.setState({ n: this.state.n + 1 });
  render() {
    const { n } = this.state;
    return <span onClick={this.increment}>{n}</span>;
  }
}
